# Command-line installation (`actingctl install`)

Workflow #421 (Alice, #405, 2026-10-24: 「你这边直接开始使用/新增命令行安装的方式就行」). The
frozen model is INST-M v1.0 (`rd-1024\install-design\model.md`). `actingctl install` installs,
updates and rolls back a **single-location root** from the command line: verified release
builds kept under the root, an in-place update with a journal, a gated rollback, the
configuration candidate, the instances, the standard packs and a fresh root. It is the
installer of a single-location root ([runtime-watchdog.md](runtime-watchdog.md), "Layout").

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
| `install\identity.json` | `actingcommand.installation-identity.v1`, the installation's number (Workflow #453 S11, [state-root-installation.md](state-root-installation.md)); not the installer's: no installer command writes, moves, renames, keeps or lists it, so an update, a rollback, an undo, `status`, `leftovers` and `place` leave it as it is | the Runtime: the first start of the installation that finds none (`.part`, flush, then a hard link that fails when the file is there, or an exclusive create where hard links are not supported; once) |

**Kept files.** On a single-location root Tools never deletes anything; `install leftovers` lists
what it kept. The single-location kept files:

- `<file>.old-<ms>`: a replaced program file, game-root item or game root;
- `<file>.rolledback-<ms>`: a file an undo moved aside;
- `actingd.config.json.prev-<ms>`: a replaced configuration (INST-3);
- `<name>.new-<ms>.part`: a file or folder an operation is staging; `<name>.abandoned-<ms>` when an
  undo abandoned it; the rescue module's `instance add` (INST-5) writes its candidate so before its
  operation opens, `<ms>` the command's own milliseconds;
- `install\current.json.finished-<ms>`: the marker of a finished operation.

`<ms>` is the operation's Unix milliseconds (`op-<ms>`), so each kept name says which operation
made it. `.old-<unix_ms>` and `.part` are names the bundle grammar reserves for exactly this
(`package.rs`, reserved ids), so they never collide with a pack directory. No installer command
touches the state root or the ledger; the rescue module's `new` (INST-6, built by Workflow #453
S4) only creates the empty state-root directory.

**The installation's entries beside the Runtime's** (Workflow #452; the coordinator's ruling of
2026-11-13 on board #452, a requirement on the slice that builds the inventory, R-15e). Directly
under a single-location root the graphical installer writes entries that belong to the
installation without being the Runtime's own: the files `acui.toml` (the console's settings of that
root), `install-wizard.log` (the graphical installer's log) and `install-wizard.config.json` (the
complete configuration it hands to `config replace`), and the directory `licenses` (the third-party
licence texts it places). The inventory, when it is built, lists none of them as `foreign`; which
other class each takes is that slice's to state. `vision` is not one of them: it is the
root-relative default of the configuration's vision root, governed by the configuration and not by
this statement. Nothing reads this list yet: no installer command of this build takes an
inventory.

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
later slice (with INST-6's fresh root, whose vision models need it). **Amended by Workflow #453
S4** (the coordinator's ruling on board #453, not the owner's words): a fresh root downloads
nothing, so `install new` and the fresh root's models leave this paragraph, which now holds for
`release stage` only. The rescue module's `new` takes a release folder or a kept build folder
that is already there (`--build`); a fresh root's vision and OCR models come by the rescue row
that places a vision root, never by a download.

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
lists no UI build. A kept UI build that verifies answers `unchanged`; one that does not refuses
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
- **Subsystems the schedules require** (Workflow #429 RB-1, since 0.12.80; register §1.514). An
  update and a rollback alike: the target build must name every subsystem that a `schedule.json` a
  start of it would read lists in `requires` ([standard-pack-schedule.md](standard-pack-schedule.md),
  "`requires`": a Runtime that does not name one refuses the file, and a start compiles the active
  catalogue generation composed from such files).
  - *What the target names*: the enabled rows of `config_manifest.subsystems` in its own
    check-config answer, the one above (the set a start of that build gives the composer); a row
    `enabled: false` names nothing. A passing answer without them (a build without the member)
    names none, so any `requires` refuses it. 0.12.77, 0.12.78 and 0.12.79 print it with a
    configuration they pass (read at their tags); 0.12.77 and 0.12.78 name neither
    `pool_list_rows` nor `catalog_business`, 0.12.79 names `pool_list_rows` and not
    `catalog_business`.
  - *A target whose check-config did not pass* (fix round 1, the coordinator's ruling): the check
    does not run, so there is neither row nor member. Its answer judged nothing (a refusal prints
    no report), and both plans refuse already for the true reason, as before: an update
    `config_refused_by_target`, a rollback the gate's part `config` (`rollback_gate_failed`), for
    which a person restores the configuration first.
  - *What is read*: the `schedule.json` of each game in use (each `game_id` of
    `policy.facts.instances`) below `standard_packs.root`; and each fragment
    (`fragments/<game>.json`) of the active generation's directory in the state root's catalogue
    store (`policy\catalogs\generations\<hex>\`), the generation each check-config answer, the
    target's and the live build's, previews as active (`policy_plan.active.catalog_hash`). Each
    file is read by the one reader of `schedule.json`: a file that is not there requires nothing,
    nor does one the reader refuses (every Runtime refuses it as well, and check-config notes it).
    **Not seen**: a generation that only a dispatch still in flight pins, which a start compiles
    too (register §1.505, "At deploy"); its hash is in the ledger alone, which the plan does not
    read.
  - *The refusal*: when a file requires a name the target does not name, or a file or the folder
    of fragments cannot be read, the plan has one more edge row, `requires`
    `next_names_required`, failed (party `R*`, source `check_config`, no range), and its document
    the member `requires_missing`, one entry per such file: `game`, `source` (`root` or
    `generation`), `catalog_hash` (a fragment's generation), `path`, `missing` (the names the
    target does not name, in the file's order) and, for a file or a folder that could not be read,
    `unread` (its failure as a warning; `game` absent for the folder, `missing` empty). The plan
    refuses `interface_edge` with that row counted; apply, which plans again, refuses the same
    before any change; a rollback's gate is not reached. `undo` judged as a rollback, `status
    --check-rollback` and a composite's automatic rollback plan the same way and carry on the
    plan's refusal alone, its outcome `install_command_refused` `interface_edge` with `count` (the
    answer of `undo`, the cause of a refused automatic rollback's `rollback_gate_failed`, `refused`
    of `status --check-rollback`), not `requires_missing`: the list is in the answer of the plan a
    person asks for (`rollback plan --to <commit>`). The two answers' active generations are both read when they differ. When
    nothing is missing there is neither row nor member: the plan is the plan it was. The check
    writes nothing and runs nothing beyond the two check-configs above.
  - *The way out* (no flag passes it): place schedules that do not require the name, let the
    Runtime of the live build compose and activate a generation without it (its workbench's next
    check after the placement), and plan again. A person sees that the state no longer holds such
    a generation in the plan itself: its `requires_missing` then lists no `generation` entry, the
    active generation being another than the `catalog_hash` it named (the Runtime's own is the
    `catalog_hash` of its schedule explanation, `ac_schedule`).
- **Ledger verify.** When the target's `ledger` range differs from the live one, apply first
  runs the target's `actingd ledger-maintenance verify --config <root>\actingd.config.json`
  (cold) and refuses `ledger_verify_failed` unless it exits 0 (the acsetup gate of
  [component-interfaces.md](component-interfaces.md), "Checks per operation"). A target from
  0.12.36 on answers these arguments with its bounded check (#422 LN-VERIFY,
  [ledger-maintenance.md](ledger-maintenance.md), "`verify` of a formal root"): what its start's
  ledger open does to authenticate the ledger, with its own decoder, in memory that does not grow
  with the history, changing nothing. It does not read an older record: a damaged one is found
  when it is read, by the background re-check or by `--full`, which the installer never passes.
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
  replaced while running: `actingctl.exe` and, since 0.12.57 (Workflow #434 LAB-5G6), the Lab's
  MCP server `actinglab-mcp.exe`, in `tools\` or `runtime\`, are renamed aside and keep running
  (listed as `running_renamed`); `adb.exe` is never moved and any other image waits
  (`running_image_blocks`, `path`, `pid`). A `.part` of another operation under
  `runtime\` or `tools\` refuses `foreign_part` (`path`).
- **Notice block** for the person's notice before the change: what changes, whether MCP servers
  must reconnect, and whether the rollback window will be bounded (a one-way ledger edge).
  `mcp_clients_reconnect` counts only a renamed image that serves MCP (an `mcp-serve` command
  line, or since 0.12.57 the image `actinglab-mcp.exe`, "Running images") whose parent process
  is not an MCP entry running under the root (`actingcommand-mcp-entry.exe`, and since 0.12.57
  `actingcommand-mcp-entry-g2.exe`): an entry switches its own child to the live build ("The MCP
  entry"; #411 P49, v0.12.25).

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
`running_renamed: [{pid, mcp_serve}]` and `next` (the undo form; `since` 0.12.36 a composite whose
start spawned leaves it out when the edges below would refuse that undo, register §1.242). When
`running_renamed` is not
empty the caveat `mcp_clients_on_previous_build` (`count`) is a `note` link. A live build equal
to the target answers done with `installation_changed: false` and changes nothing. A step that
fails after the first change on disk leaves the journal and `current.json` and ends
`install_operation_interrupted` (fatal, `operation`, `install_operation_id`) with the step's
failure as its link: the root cannot run normally until `resume` or `undo`.

**Rollback** `--to <commit>` is an update to a kept build, with the rollback gate (below) in its
plan and evaluated again under the writer lock immediately before the first change. A failed
gate refuses `rollback_gate_failed` (`rollback_gate_parts`, and `ledger_sequence` when the
target's reader refused a page): forward fix only. Success answers done, as an update does. A
failed interface edge refuses `interface_edge` whatever the ledger holds; `--accept-ledger-one-way`
governs only the upgrade direction, and once the build has started `undo` cannot go around the
edges or the gate ("Undo and resume", below).

**Undo and resume.**

- `undo` reverses an unfinished operation, or the last finished one when no operation followed
  it; once a Runtime of the undone build has started, inside the operation or after it (below), the
  undo is a rollback and the interface edges, then the whole rollback gate, must pass; with none
  started neither is judged. Every step is reversed as the step table says; every displaced file
  is kept. Result: done (`install_operation_id`).
- `resume` continues an unfinished operation: each step's postcondition is checked on disk, a
  step already true is done, a missing half is finished. Result: done (`install_operation_id`)
  with the operation's own result members, and the operation's own result as its `related`
  link; an operation that ends refused answers its refusal. An operation whose end (or its
  undo's end) is in its journal while `current.json` still names it (a crash or a failed rename
  between the journal's last write and the rename of `current.json`) is not run again: `resume`
  only renames `current.json` `current.json.finished-<ms>` and answers the end it recorded (the
  journal's `result`; done of `install_undo` for an undo) as its `related` link (INST-1b-F fix
  round 1, review S1 F5); of the undo of a `pack_place` operation, with the
  `schedule_changes` its undo record holds and their warnings (Workflow #463 S3-18 fix round 1,
  "The packages the undo of a placement makes enter or leave scheduling"). A failure of that last
  write or rename is
  `install_operation_interrupted`, so the person knows to resume.
- `undo` of a finished update or rollback reads the state root's owner journal: a journal that
  cannot be read or whose last record this installer does not recognise refuses
  `processes_unreadable`, and one a running owner holds locked refuses `owner_lock_held`;
  neither counts as "no owner started" (review S1 F6).
- **An undo once the build has started** (`since` 0.12.36; register §1.242). `undo` of an update or
  rollback, finished or not, is judged as a rollback to the build before it whenever a Runtime of
  the undone build has been started; a composite's own start counts, so stopping that Runtime by
  any means but an install command does not get around the judgment. First the interface edges:
  the rows of "The rollback gate" part 1 that the declared ranges, the packs and the
  configuration's keys decide (`--accept-ledger-one-way` is never given), so an undo across
  `previous_max_in_next` is refused whatever the ledger holds. The two builds' ranges are those
  the operation's journalled plan read from their manifests; a journal without them refuses
  `state_invalid`. Failed edges refuse `interface_edge` (`commit_sha` the build before the
  operation, `count` the failed rows; the result's `edges`, each row as a plan's, with the parties'
  ranges). Then the whole rollback gate (below): a failed part refuses `rollback_gate_failed`
  (`rollback_gate_parts`, `ledger_sequence`). Both refuse before the writer lock with nothing
  changed, and leave a forward fix or, for an unfinished operation, `resume`. A running actingd
  still refuses first (`runtime_running`). An undo already under way goes on unjudged, and so does
  the undo of an automatic rollback that began, since it records itself only once its gate passed.
  - **What counts as started.** A `start` step of the operation, not an automatic rollback's, whose
    evidence shows a process spawned (an attempt ready or ended, a progress sample, an owner a
    resumed start found), including one that exited during startup or was left running at its
    deadline; a `start` step begun with no evidence while the operation is unfinished, which a crash
    before its first progress leaves and the journal cannot tell from no spawn; or an owner record
    of a finished operation that began after the operation did, whether inside it or after its end.
  - **What does not.** A plain update or rollback, a composite interrupted before its `start`, a
    `start` refused before any spawn (`guard_failed`, `runtime_running`, `owner_lock_held`, a run
    lock not taken: the operation ended with no evidence on the step), and a spawn that failed on
    every attempt. These undo as they did.
  - **The composite's failures.** A failed post-start check or a new start that exited begins the
    automatic rollback. When its gate refuses it, by the edges or any other part
    (`rollback_gate_failed`, the new build and the hold kept), the undo of that operation is judged
    the same way once its Runtime has stopped: a refused automatic rollback stays a forward fix
    until the gate passes. A rollback that ran (`deploy_rolled_back`) leaves the previous build
    live: the operation has nothing to undo (`actingctl_usage_invalid`). One interrupted past its
    gate is undone unjudged, its files perhaps reversed already.
- An operation id that names no journal, an operation `resume` cannot continue (finished) or one
  `undo` may not reverse (not the last) is `actingctl_usage_invalid` (`argument` `--operation`,
  `argument_problem` `unusable`).

**Status** (done) answers the layout, the live and kept builds, the pack
register, the committed configuration SHA-256 and whether a candidate exists, the unfinished
operation, the last finished operations, the leftovers count, and the rollback window's cheap
parts (`rollback_window`, one row `{commit, ledger_edge_passes}` per kept build but the live one;
whether an owner started since the live build's operation). A row tells **the ledger's edge
only** (the live build's `ledger` `max` within the kept build's range, as the plan's
`previous_max_in_next`), not the other edges: a plan toward a kept build whose row is `true` can
still be refused by another edge. So since 0.12.75 (#480 PG-2e, register §1.485) a kept build
whose tools end below `runtime-client` 12 (0.12.72, 0.12.71) passes the ledger's edge, its row is
`true`, and its rollback plan is refused by `installer_meets_next`. `--operation <id>` answers
that journal. `--check-rollback` evaluates the
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
  during the drains is what the start restores), and (Workflow #454, 0.12.59) whether that
  Runtime is in its root's first epoch, from its program fact `config.start`, read with the
  configuration facts (the close step's evidence `first_epoch`, "The post-start checks
  (INST-2)"). The closed start's terminal outcome is read
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
  `release-hold`. This build still names `release-hold` beside `install start` there
  (`hold_kept`).
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
  path, mcp_serve}]`, the running processes under the root that serve MCP (an `mcp-serve`
  command line, or since 0.12.57 the image `actinglab-mcp.exe`, "Running images") whose image an
  update renamed aside (`<file>.old-<ms>`); their clients reconnect, except an MCP entry's child,
  which its entry switches.

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

The `post_check` step's evidence is `{commit, checks, failed}`: every row, and the ids of the
fail-class checks that failed. A step whose checks pass, and one that begins an automatic rollback
(with the member `rollback`), is `done` with it as before. Workflow #454 (0.12.59): a step whose
failed check ends the command `post_check_failed` (a composite rollback, the rescue module's
`config replace`, `config restore` and `instance add`) is closed `done` with the same evidence in
the write that records the operation's result, so the record says which row failed and what it
read; before, it stayed `started` with no evidence, the rows only in the answer's `post_check` and
`failed_checks`. A crash before that write leaves the step `started`, and `install resume` runs the
checks again. So a `post_check` step in state `done` says only that the checks ran to their end;
whether they passed is its `failed` member (empty: every fail-class check passed). That is true of
every `done` step, also where a failed check does not end the command `post_check_failed`: the
checks of `install start` (a failed one is the caveat `post_check_mismatch`) and those of an
automatic rollback's own start (the command still ends `deploy_rolled_back`, its hold kept) close
their step `done` with a `failed` that is not empty, and a step that begins an automatic rollback
is `done` with its `failed` and `rollback`.

Workflow #454 (0.12.59; register §1.348; fix 1): the close also records whether the Runtime it
stops is in its root's first epoch, in its step's evidence; S0 keeps its four members. Just before
`request-shutdown`, S0's second read of the pauses and instructions reads that Runtime's status
and its configuration facts, and with them its program fact `config.start` (one row `{owner_epoch,
first_start, tagged_state}`, [runtime-fact-store.md](runtime-fact-store.md), "Producers"; no other
read and no other connection). The close's evidence then carries:

- `first_epoch`: `true` when the fact is of S0's owner epoch and says `first_start: true` with
  `tagged_state` `seed` (that start found no earlier start record, so it wrote no seeding marker,
  [ledger-observer.md](ledger-observer.md) §11.5) and no other journal of the root records an S0
  of another owner epoch; `false` otherwise.
- `first_epoch_unread`, only when the close could not read the fact: the code of the status or
  fact read that failed (a read that timed out included), `runtime_not_running` when no Runtime
  answered the status read, or `fact_absent` when the Runtime records no `config.start` (every
  build before 0.12.59).
- `first_epoch_guard`, only when the fact said the first epoch but the root's own journals say
  otherwise (the second guard): `{operation_id, owner_epoch}` of the first operation whose S0 names
  another owner epoch, `{operation_id, journal_unread}` of a journal that could not be read, or
  `{journals_unlisted}` when the journals could not be listed (as in `first_epoch_unread`, the
  member names what could not be read and holds the failed read's code); `first_epoch` is then
  `false`. The guard sees only the Runtimes an operation of this installer stopped.

With `first_epoch` the root's next start seeds by design, and `no_seeding` passes it; in every
other case (a later start, a fact of another epoch, which an older build leaves in place, no fact,
a read that failed) `no_seeding` requires `load` as before. The checks of the operation's own start
read it from the newest close step before them that is not an automatic rollback's; `install
start` reads it from the close of the operation that stopped the Runtime (`last_stop`, carried in
its journal's `inputs` as `"first_epoch": true`), and only when that operation began no start of
its own after its close. An automatic rollback's checks judge without it (their start follows the
new build's Runtime, not S0's). Two cases judge a true first epoch not first, the safe side: (a) a
close that finds no Runtime answering its status read, as when a close is resumed after the
Runtime it stopped is gone, records `first_epoch` `false` with `first_epoch_unread`
`runtime_not_running` and takes no fact; (b) an operation with a `start` step in any state other
than `planned` counts as having started the Runtime itself (that start may have written the
seeding marker), also when its start never reached a Runtime, and carries no `first_epoch` to
`install start`. In both the root's next start seeds and fails `no_seeding` as before Workflow
#454, with what a failed check does in that command; that start found the first epoch's start
record, so it wrote the seeding marker, and a start after it loads: the person runs the command
once more, or, where the Runtime is down, starts it with `install start` (the rescue module's
`up`).

| id | Judged | Fail-class |
|---|---|---|
| `ready` | the start reached readiness (`ready_ms`), and the owner it made ready still answers (a Runtime gone or restarted at check time fails) | yes |
| `open` | the open's phases sum within its deadline and no `ledger_open_slow` (report, else the start log) | no |
| `no_seeding` | a root that ran before (an S0) loaded its tagged state, not seeded again (`{tagged_state}`); with the close's `first_epoch` (Workflow #454) `seed` passes as `load` does (`{tagged_state, first_epoch: true}`); without an S0 it passes (`{tagged_state, earlier_runtime: false}`) | yes |
| `bindings` | every instance alias of the configuration the start was given bound: the root's file as the checks read it (Workflow #454, 0.12.59: after the swap of `config replace`, `config restore` and `instance add`, the new file; before, the file as the command read it before the swap) | yes |
| `packs` | every game of the pack register registered, with its pack count | yes |
| `pauses` | S0's pauses, plus the hold on every instance it took; (Workflow #453 S3, since 0.12.60) an instance pause `instance.added` on an instance that S0 and the hold leave unpaused and whose entry of the configuration the start was given declares `start_paused: true` is the Runtime's own hold at a first start ([scheduling-pause.md](scheduling-pause.md), "Pauses held at a first start") and is expected, those instances named in the row's `start_paused` (absent when none; any other extra pause still fails the row) | yes |
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
actinglab new <new_root> --build <kept build dir or verified release dir> [--new-state-root <dir>] [--mumu-root <dir>] [--lab-authoring true|false] [--watchdog]
```

(The last line is the Lab's rescue row, Workflow #453 S4: `actingctl install` has no `new`.)

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
- **Instances, the first slice** (Workflow #421 INST-5, since 0.12.52; register §1.300; the
  coordinator's work order of 11-11). Only the Lab's rescue module serves it, as `instance add
  <alias> --game <g> --server <s> --mumu-index <n> --touch-backend <b> --capture-backend <b>
  [--application-id <id>]`, every parameter but the application id required and none with a
  default; it adds one emulator instance, bound by its index. Not in this slice: `actingctl
  install instance add` and `instance remove`, INST-3's candidate commands, a handset, and INST-R
  (the instance written in one place). It does not wait for INST-3 and INST-R: it composes and
  commits the candidate itself, in one call.
  - **The application id** (Workflow #452, since 0.12.59; register §1.300, "Fix #452"). The
    configuration's assembly requires an `application_id` of every entry bound by discovery
    (`application_identity_missing`), so the entry carries one: `--application-id <id>` when it
    is given, else the id the game's root under `standard_packs.root` declares for the server,
    `<game>\applications.json`, `servers.<server>.application_id` (the member the Runtime's idle
    action reads; a leading byte-order mark is skipped). A given id is checked by the assembly's
    own rule and no stricter one: not blank, at most 256 bytes, no control character (otherwise
    `actingctl_usage_invalid`, `unusable`, `--application-id`; one of another type
    `wrong_type`). A given id is written whether or not the root declares another; no refusal
    compares the two. The Runtime later judges an instance by the id its game's root declares
    only in the idle action: its `close_app` (and `auto` when it takes that tier) checks that
    the instance's configured application is the declared one and, when they differ, refuses
    before its stop request with `invalid_application_identity` (`alias`, the declared
    `application_id`), recorded for that instance alone; the application is left running and no
    wake is owed.
  - **Refused before anything is written**, each `instance_invalid`: an alias the configuration
    has, an entry of `instances` or a row of `policy.facts.instances` (`alias`); an emulator index
    an entry binds, `instance_index` (`alias`, `instance_index`, the entry's alias as `aliases`); a
    game whose root under `standard_packs.root` holds no `bundle.json` the contract crate reads
    (`game`, `path`, the read's failure as the cause; `config_key` `standard_packs.root` when the
    configuration names no such root), or a server no pack of that bundle names (`game`,
    `server`, `path`); (Workflow #452) no `--application-id` and no id the game's root declares
    for the server (no `applications.json`, one that does not read or is no JSON, no member of the
    server, no `application_id` there, or one the assembly's rule refuses), with `game`, `server`,
    the file's `path` and the read's failure as the cause when there is one; a configuration
    without the list `instances` or `policy.facts.instances` (`config_key`, the list's key);
    (Workflow #453 S1) a configuration without `policy`, absent or `null` (`config_key`
    `policy`); with `policy.facts.instances` empty, the refusals of a first row (below). A
    parameter missing or malformed is `actingctl_usage_invalid`. With no Runtime answering, the
    stopped rule refuses first ("While the Runtime is stopped", below; before Workflow #453 S3 it
    refused `runtime_not_running`).
  - **The candidate**: the selected configuration with the entry `{alias, instance_id,
    application_id, instance_index, touch_backend, capture_backend}` appended to `instances` and
    the row
    `{instance_id: <alias>, server_id, game_id, host_id, available: true,
    capability_operation_ids, preferred_task_ids: []}` appended to `policy.facts.instances`, the
    host and the capabilities those of a row of the same game and server, else of the same game,
    else of the first row (a first row's, below, when there is none); both places, as the
    configuration's assembly requires until INST-R (`policy_instance_set_mismatch`); every other
    member as it was, in its order, with the live file's line ends. The `instance_id` is minted
    (`instance_<32 hex>`) and is none the live file names and none a journal of the root recorded
    as added.
  - **A first row** (Workflow #453 S1, since 0.12.60; register §1.357; the coordinator's work
    order of 11-14 on board #453, not the owner's words). When `policy.facts.instances` is empty,
    as on a fresh root, the row is composed from the configuration alone, after every check above
    and at the place the copied row's check stood. Its host is the one
    `policy.resources.hosts[].host_id`: no host list, an empty one, several hosts, or one host
    without a `host_id` text are refused `instance_invalid` with `config_key`
    `policy.resources.hosts`. Its capabilities are exactly the composer's one operation
    (`operation.package.run`, the policy crate's `STANDARD_PACK_OPERATION_ID`), and only in
    composer mode (`policy.catalog` absent or `null`, as the configuration's assembly reads it):
    in catalog mode the catalog, not the installer, says what an instance can run, so an empty
    list is refused as before, `instance_invalid` with `config_key` `policy.facts.instances`.
    The candidate then passes the assembly's policy check (the set of
    `policy.facts.instances[].instance_id` equals the configured aliases, every `host_id` is a
    declared host) by construction. Nothing else differs: the candidate file, its place, the
    commit and every other refusal are those of a configuration with rows, and a configuration
    with rows composes the same bytes as before (a commit while the Runtime is stopped came with
    Workflow #453 S3, below). `install new` is what writes a first `policy`; this slice writes
    none (Workflow #453 S4: the rescue module's `new`, "Fresh root").
  - **The commit** is `config replace`'s ("The Lab's rescue module"): the candidate, written
    beside the live file as `actingd.config.json.new-<ms>.part` (`<ms>` the command's own
    milliseconds) so that its relative paths resolve as the live file's, passes the live build's
    check-config (`--install-root`; a refusal is `config_refused_by_target` with check-config's
    outcome as its cause, whole); then, while a Runtime runs (without one: "While the Runtime is
    stopped", below), the hold of every unpaused instance (`lab-instance-<ms>`,
    every other pause kept) and its global pause (INST-5b, below), the close, the swap with the
    replaced file kept as
    `actingd.config.json.prev-<ms>` (the candidate is the swap's own part, so nothing else is
    staged), the start and its post-start checks: one journal of kind `config_replace` with the
    initiator and the input `instance_add` (what was added, its `instance_id`, the pause reason,
    `capabilities_from`, the alias of the row whose host and capabilities were taken; since
    Workflow #452 also `application_id` and `application_id_from`, `given` or `declared`; since
    Workflow #453 S1 `capabilities_from` is `null` for a first row, the member always present).
    The answer's `added` is the same record.
  - **The new instance starts paused** (the coordinator's ruling of 11-11, not Alice's words):
    with a Runtime running, after the start, before `alive` and the release of the hold, it is
    paused with its own reason
    `instance.added`, a pause reason name ([scheduling-pause.md](scheduling-pause.md):
    `[a-z0-9_.-]`, 1 to 64 bytes), drained as every pause is; the release hands back only the
    hold's own pauses, so it stays paused until a person lifts it (the console's resume,
    `actingctl resume --state-root <s> --instance <alias>`, or since LAB-6S1b the Lab's
    `resume <alias>`) when the account is ready. Not
    registered by the started Runtime: from 0.12.59 (Workflow #454) the post-start check
    `bindings`, which judges the new file, fails (`post_check_failed`, `failed_checks`
    `bindings`, the row's `missing` the alias), the hold kept, and the pause is tried as after
    every stop after the swap (below); its pause refused: `post_check_failed` (`alias`, `count`
    1), the hold kept, and `next` names `pause_added`, the core pause as actingctl runs it
    (`actingctl pause --state-root <s> --instance <alias> --reason instance.added`). An
    operation that stops after the swap pauses it the same way while a Runtime answers. A stopped
    Runtime takes no pause of an alias it did not register, so the instance's own pause comes only
    after the new Runtime's readiness; the global pause below holds it until then.
  - **No moment unpaused** (Workflow #421 INST-5b, since 0.12.53; register §1.300, "INST-5b"; the
    coordinator's ruling of 11-11, not Alice's words). The hold's pause step, after its instance
    pauses and before the close, also sets one global pause with the hold's own reason
    (`lab-instance-<ms>`: a global pause and the instance pauses are separate scopes, each with
    its own revision, so one reason names the whole hold), unless a global pause is in effect
    (anyone's: it sets none and leaves that one alone, reason and revision, since it holds every
    instance as well). The Runtime persists it and restores it at its start before any physical
    instance is prepared ([scheduling-pause.md](scheduling-pause.md), "Persistence, no expiry"),
    so every instance, the new one too, is held from the new Runtime's readiness to the new
    instance's own pause. The step's intent asks for it (`global: true`), and its evidence records
    it as the operation's own (`own_pauses.global`, its revision, in `own_pauses.owner_epoch`); the
    close's second read of S0 lists it, which the start restores, so the post-start check `pauses`
    expects it. Only after the new instance's own pause does the release hand it back, first, then
    the hold's instance pauses (the release's `global_resumed: true`). A Runtime that does not
    answer when it is to be set, or that refuses it, refuses the pause step
    (`runtime_not_running`; `runtime_stop_failed`, the Runtime's failure as the cause) before
    anything changed on disk, the hold's own pauses handed back. Every failure after it keeps it as
    the hold is kept: when the new instance's pause fails, the command ends `post_check_failed`
    with both kept and `next` names `pause_added` and `actingctl_resume_global` (the core global
    resume, "Ways out of a kept hold") beside `release-hold <its reason>`, which hands back the
    global pause and the hold's instance pauses and nothing else. `config replace`, `config
    restore` and every other composite set no global pause.
  - **Its answer**: `install_command_done` (`install_instance_add`, `install_operation_id`,
    `alias`, `game`), the steps `check_config`, `swap`, `pause_added`, `alive` and `release`, the
    `backup` and `added` (as the journal's input records it, `application_id` and
    `application_id_from` included). A candidate refused after it was written (by check-config,
    or a Runtime gone) stays beside the live file, kept as every part: nothing deletes it, and
    `leftovers` lists it.
  - **While the Runtime is stopped** (Workflow #453 S3, since 0.12.60; register §1.365; the
    coordinator's rulings on board #453, not the owner's words). With no Runtime answering the
    status read, `instance add` no longer refuses `runtime_not_running`. Before the candidate is
    written it applies the stopped rule the update applies after its close and `install start`
    before it spawns (one function, `install::launch::refuse_running`, which `place` while the
    Runtime is stopped applies too): an `actingcommand-actingd.exe` running under the root refuses
    `runtime_running` (`path`, `pid`), the state root's owner lock held `owner_lock_held`
    (`path`), a probe that cannot decide `processes_unreadable` (the failed read as the cause); a
    status read that fails is `processes_unreadable` as before. Each leaves the root as it was.
    The candidate is the one above with the new entry, and no other, carrying `start_paused: true`
    as its last member ([runtime-config.md](runtime-config.md), "An entry that starts paused").
    The live build's check-config judges it as above, so a live build before 0.12.60 (before
    Workflow #453 S2), which does not know the member, refuses it: `config_refused_by_target`
    with check-config's own refusal (`config_decode_failed`) as its cause, nothing swapped, the
    part kept and listed by `leftovers`. The commit takes the writer lock, refuses an unfinished
    operation and applies the stopped rule again (the candidate's member keeps it on this path, so
    a Runtime that came up since the first read is refused here, never held), then opens one
    `config_replace` operation without hold, S0 or guards: the steps `stage_file` (nothing to
    copy: the candidate is the part) and `swap_file` (the live file kept as
    `actingd.config.json.prev-<ms>`, the part renamed in, its SHA-256 checked), `current.json`
    retired at the end; its input `instance_add` records `start_paused: true` (absent for a
    candidate committed with a Runtime). Nothing is started and nothing is paused: the Runtime's
    next start holds the instance with `instance.added` ([scheduling-pause.md](scheduling-pause.md),
    "Pauses held at a first start") until a person resumes it, and the post-start row `pauses` of
    `install start` expects that pause ("The post-start checks (INST-2)"). The answer:
    `install_command_done` (`install_instance_add`, `install_operation_id`, `alias`, `game`), the
    steps `check_config` and `swap` (no `pause_added`), `backup`, `runtime_running: false` and
    `added`. A failure after the first rename is `install_operation_interrupted`, the journal and
    `current.json` kept for `resume` and `undo`; while it is unfinished the watchdog stands aside
    (`config_replace` is off row 7a″'s pass list). The `resume` and the `undo` of a
    `config_replace` begun while no Runtime answered (its journal holds no hold and no S0: this
    path, and `config replace` and `config restore` while stopped) take the writer lock and apply
    the same stopped rule before anything is moved (`owner_lock_held`, `runtime_running`,
    `processes_unreadable` of `install_resume` or `install_undo`; no status read comes first, so a
    running Runtime is `owner_lock_held`), as those of `place` while stopped do; those of a
    `config_replace` begun with a Runtime are unchanged. A crash between the swap's two renames
    leaves no live configuration: `undo` (which then reads the file the swap kept, the input
    `backup`) puts it back, while `resume` cannot load the root (`config_unreadable`), a known
    limit of every `config_replace` that a later slice lifts. With a Runtime answering, the
    candidate, its commit and its answer are as before: no entry gains the member. Kept off the
    production root until 0.12.62 ("Journals across versions").
  - **Taking it back**: `config restore <backup>` puts the kept file back by the same path. Its
    post-start checks judge the pauses from before the restore, so the instance it removes shows
    as a `pauses` failure (the hold's pause on it is not restored): it ends `post_check_failed`
    with its hold kept, the previous configuration running, and `release-hold <its reason>` hands
    the hold back. `bindings` judges the restored file (Workflow #454, 0.12.59; before, the file
    from before the restore, and the removed instance failed it too). `instance remove` (a later
    slice) is the clean way.
- **Packs** (INST-4). Plan with the contract crate's own readers (`BundleIndex::validate`,
  `read_standard_pack_root`), the package interface edge, the delta per item and the
  Runtime-semantic rules (`pack_binding_broken`, `pack_maintenance_drift`, the caveat
  `pack_restart_needed`); place cold or hot in the reader's order, each item `.part` →
  `.old-<ms>` → in place; the register records game, source repository, commit, tag and the
  declared `package` range, never a digest (done; `instances_not_idle`, the instances staying
  paused; the caveat `pack_full_check_skipped`). `remove` renames the game root
  `packages\<game>.old-<ms>`; `restore` puts one pack operation's kept items back.
- **Fresh root** (INST-6; built by Workflow #453 S4, since 0.12.60; register, "Slice #453 S4"; the
  coordinator's rulings on board #453, not the owner's words). Only the Lab's rescue module serves
  it, as the row `new <new_root> --build <dir> [--new-state-root <dir>] [--mumu-root <dir>]
  [--lab-authoring true|false] [--watchdog]`, so that the Lab's command line and its MCP server both
  have it; `actingctl install` gains nothing (no `install new`). The positional is `new_root` and
  the state root's option is `--new-state-root` (parameter `new_state_root`), never `--state-root`:
  the module refuses a parameter named `root` or `state_root` on every row (the coordinator
  accepted the name after the review); the row declares every parameter. It is the one rescue
  row served without a located root: it acts on the root it names, not on the root of the program
  that runs it, so the release's own `actinglab.exe` (the Tools zip unpacked anywhere) and the
  Lab's MCP server run it before any root exists. Nothing is started; the vision and OCR models
  are not this row's (the vision root is placed by its own rescue row, never by a download).
  - **`--build`**: a release folder that verifies, read by the release reader as `release verify`
    reads it (both zips, `SHA256SUMS`, and `release-facts.json` in the folder when it is there;
    without it the answer carries the caveat `release_unconfirmed` as a `note` and the kept build
    is recorded `origin` `local_build`), its zips extracted into a new work folder that is left in
    place and named (`work_dir`); or a kept build folder (`runtime\BUILD-MANIFEST.json` in it,
    checked against its own manifests, its `release.json` taken with it). The build's Runtime
    member must carry the first configuration's template, `actingd.config.first.json`.
  - **The state root**: `--new-state-root`, else `<new_root>\state`, written absolute.
  - **Refused before anything is written** (`install_command_refused` of `install_new`, each its
    own token): a `new_root` that exists and is not an empty folder, `root_not_empty` (`path`; a
    root that holds an unfinished operation of `new` also names it, `install_operation_id`, and
    `next` the ways on); a state root that exists and is not an empty folder,
    `state_root_not_empty` (`path`); a `--build` that does not verify, the release reader's
    `release_check_failed` (`release_checks`, the reader's refusal as the cause, its `failures`
    beside), or for a kept build folder, a UI release or a build without the template
    `build_invalid` (`path`, `count` and `files` for files that differ); an empty `actingd-config`
    edge `I ∩ R` between the running installer and the build, `interface_edge` (`commit_sha`,
    `count`, the row in `edges`; "Interface checks"). A refusal that comes after the release
    reader unpacked a release folder (the edge, a UI release, a build without the template, a
    facts file or a manifest that cannot be read) names that work folder, `work_dir`, left in
    place (fix round 1); nothing under the new root or the state root is written.
  - **One journalled operation** of kind `new` (off row 7a″'s pass list, so no watchdog starts a
    Runtime on the root while it is unfinished; a watchdog never starts a root that never started),
    its initiator recorded, under `install\writer.lock`, its steps in this order: `copy_build` (the
    build's listed files and both manifests into `install\builds\<c>.new-<ms>.part`, with its
    `release.json`), `verify_tree` of that part, `place_build` (renamed `install\builds\<c>`), one
    `stage_file` and then one `swap_file` for every file of the kept build, both manifests included,
    into `tools\` and `runtime\` (all new, no `.old`), `verify_tree` of the live tree,
    `write_config`, `create_state_root`, `create_vision_root` (fix round 1, below) and
    `record_state` (`state.json` v1: the live build
    installed by this operation, the kept build with `origin` `release` or `local_build` and its
    tag, `packs: []`, the configuration's `config_sha256`). After the first write a failed step
    ends `install_operation_interrupted`, the journal and `current.json` kept, `next` naming
    `actingctl install resume|undo --root <new_root> --operation <id>` (until LAB-7; any build of
    this release or later runs them, and once the configuration is in place the new root's own
    module answers `op resume` and `undo`). `resume` continues each step by its postcondition;
    `undo` (the unfinished operation, or the last finished one, under the writer lock and the
    stopped rule: `owner_lock_held`, `runtime_running`, `processes_unreadable`) sets each written
    item aside from the last, `state.json.rolledback-<ms2>`, the vision root's folder as
    `vision.rolledback-<ms2>`, the state root's folder as
    `<folder>.rolledback-<ms2>` when the operation made it (one that was there, empty, stays),
    `actingd.config.json.rolledback-<ms2>` and the part `actingd.config.json.abandoned-<ms2>`, each
    program file `.rolledback-<ms2>`, each part `.abandoned-<ms2>`, the kept build
    `<c>.rolledback-<ms2>`: nothing is deleted, and the root's folder, its `install\` and the
    journal stay.
  - **The first configuration** comes from the template the Runtime member ships beside the
    example, `actingd.config.first.json` (the example is unchanged: CI starts the Runtime from it):
    the example's members, a composer-mode `policy` (no `catalog`) with one host and no instance,
    and `standard_packs.root` `packages`, relative to the configuration's folder, the new root.
    The host's `cpu_available_milli`, `gpu_available_milli` and `io_available_milli` are 1000
    each, the most a host may declare: the evaluator's placement check only asks that a task's own
    load fit them, and the performance controller overlays a host's responsiveness, third-party
    pressure and heavy-dispatch budget, never these. The command fills only `state_root`
    (absolute), `secret_fingerprint_salt` (32 bytes of the operating system's generator written
    as 64 lowercase hexadecimal digits; never printed, journalled, diffed, logged or answered),
    `lab_module_authoring` (`--lab-authoring`; absent: `true`) and `mumu_root` when
    `--mumu-root` is given; nothing in Tools names a configuration value. `write_config` writes it
    beside the live file as `actingd.config.json.new-<ms>.part` (a part an earlier attempt wrote
    is checked as it is, its salt kept), runs the new live build's check-config with
    `--install-root <new_root>` and renames it `actingd.config.json`; a refusal ends the operation
    refused, `config_refused_by_target` (`path` the part, check-config's outcome as the cause),
    the part kept, `next` naming its undo; a check-config that does not answer is
    `install_operation_interrupted` with `check_config_unanswered`.
  - **Not this command's** (Workflow #453 S11, [state-root-installation.md](state-root-installation.md)):
    `new` mints no installation identity (`install\identity.json`) and writes no state-root record
    (`installation.json`). It creates the state root's folder, empty, and writes `install\state.json`,
    which makes the root an installation root: its first start mints the identity and claims the
    state root.
  - **The empty vision root** (fix round 1; the coordinator's ruling of 11-14 after the review of
    `vision place`, not the owner's words). The rescue row that places a vision root refuses one
    that does not exist and never creates it, so `new` creates the vision root a start resolves by
    default, `<new_root>\vision` ([vision-model-folders.md](vision-model-folders.md), "Vision
    root": a `vision` section without `root`, a daemon run from an install root), empty, as the
    step `create_vision_root`, when the template's `vision` section names no `root` of its own (the
    shipped template has no `vision` section). It changes no start: a configuration without a
    `vision` section is never resolved against it, and with one, a missing root and an empty one
    are refused alike (`vision_root_unavailable`: the start lists `models\`, which neither holds).
  - **`--watchdog`**: after the operation ended and the writer lock is released, the rescue row
    `watchdog install` for the new root, last; it registers the task and starts nothing (a root
    that never started stays down, [runtime-watchdog.md](runtime-watchdog.md), row 3). Its report
    is the answer's `watchdog`; a registration that failed is the answer's top, the root's members
    beside it, and `next` names the row again.
  - **Its answer**: `install_command_done` (`install_new`, `install_operation_id`, `commit_sha`,
    `installation_changed: true`) with `operation_id`, `root`, `state_root`, `config_sha256`,
    `lab_options_enabled` (`["lab_module_authoring"]` or `[]`), `build` (`commit`, `origin`, `tag`,
    `source`), `check_config`, `work_dir` for a release folder, and `next` naming the rows that
    follow, each run by the new root's own `tools\actinglab.exe`: `place` (standard packs),
    `instance add` and `up`.
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
  the CLI returns it, neither recomputed nor filtered. Workflow #453 S4 built the command as the
  rescue row `new` ("Fresh root" above): it asks nothing and takes `--lab-authoring true|false`
  only, absent `true`; `lab_module_debugging` is retired ([lab-two-entries.md](lab-two-entries.md)
  §8.3) and `--lab-debugging` is refused as an unknown option, so a wizard passes
  `--lab-authoring` alone. No flag of an existing command changes.

## The Lab's rescue module (Workflow #434 LAB-3, since 0.12.48)

[lab-two-entries.md](lab-two-entries.md) §5 to §7; the rows of module `rescue` in
[lab-operations.json](lab-operations.json); register §1.287, "Slice LAB-3". The installer's code
moved from `apps/actingctl` to `crates/lab-rescue` and runs with one of two faces. **`actingctl
install` keeps the face this document describes, every option, output, code and
exit, until LAB-7 deletes it**; a journal it writes is byte for byte what it wrote before. The
Lab's rescue module (`actinglab <row>`, `actinglab serve --stdio`) runs the same commands with
the rescue face:

- **No root word.** It acts on the root of the `actinglab.exe` that runs it (`<root>\tools\` until
  LAB-7 moves it to `runtime\`); `--root` and `--state-root` are refused as unknown arguments.
- **The initiator.** Every journal an operation of the module writes has `initiator` (§4.3 of the
  contract's words) from before its first step; an `undo` records its own initiator in the undo
  record; a resumed operation keeps the face its journal began with. `unlock` passes the initiator
  as actingd's `--actor`.
- **No confirmation parameters.** `--plan <id>` for an apply, `--accept-ledger-one-way`,
  `--known-pause`, `--ack-guard`, `--ui-confirmed` and `--via` are refused
  (`actingctl_usage_invalid`, `argument_problem` `unknown`). `deploy`, `rollback`, `config replace`
  and `place` plan and apply in one call under the writer lock; `--plan` (a switch) answers the
  plan only. A one-way ledger edge is journalled `one_way: true` instead of acknowledged. `place`'s
  `--authorization` and `--enables` (Workflow #463 S3-2, since 0.12.64; "The authorization a
  placement names", below) are statements of the caller that the tool records, not confirmations
  it asks for.
- **Every pause is known.** A hold takes every pause it finds on an affected instance as known,
  keeps it and lists it; a global pause too is kept and listed (the `foreign_pauses` row notes it)
  and the stop goes on. The hold's reason is the module's `lab-<operation>-<unix ms>` unless the
  caller names one (`down --reason`; `deploy` takes none).
- **Only its own hold** (LAB-3 fix 1 and 2, lab-two-entries.md §5). A hold of the module never
  takes a pause it did not set: `down --reason <r>` with a reason an instance already shows refuses
  `hold_not_owned` (`hold_reason`, `aliases`) before anything is paused. What is the module's own
  comes from the operation's journal alone: the reason its pause step took and the instances that
  step paused (each pause the Runtime accepted, `drained`, and the instances its evidence record
  `own_pauses` shows paused with the reason, with the owner epoch and revisions as evidence).
  `release-hold <reason>` (the reason is required) hands back only a hold that a journal of the
  module in this root recorded in its pause step, and of it only the instances that step paused,
  each only while its pause shows that reason now, in whatever owner epoch the Runtime runs; any
  other reason (another party's, `alice-116-scheduler-stop` say) refuses `hold_not_owned` and
  nothing is resumed. No alias the step did not pause and no instance whose pause shows another
  reason is ever resumed; an instance the hold was to pause that shows its reason though the step
  did not pause it is left and named by the same refusal (`left`). Every composite that releases
  (`deploy`, `rollback`, `config replace`, `config restore`, `place`, and an automatic rollback's
  release) follows the same rule, and no answer's `next` names a `release-hold` of a hold the
  module does not own. A hold whose ownership the module cannot read (the reason does not parse,
  the root or its operations folder cannot be read) is left out of `next` all the same, and the
  answer says so (fix 3): the note `install_caveat_noted` of `install_release_hold` with
  `install_caveat` `hold_ownership_unconfirmed`, `hold_reason` when the reason parses, the read
  failure as its cause. `actingctl install release-hold` and `install stop` are unchanged.
  Workflow #421 INST-5b: a global pause is the module's own only when the pause step of such a
  journal recorded it (`own_pauses.global`, only `instance add`'s does) and it shows the hold's
  reason now; in the owner epoch the step recorded, only at the revision it recorded, in a later
  one (the restore numbers it 1) by its reason. Every hand-back of the hold (`release-hold`, a
  composite's release, the hand-back after a refusal) resumes it first, by the owner epoch and
  revision the Runtime shows; no other global pause is ever touched.
- **Why the reason and the alias, not the revision** (fix 2). A changed owner epoch alone never
  denies the hand-back: the real Runtime's restore does not keep a pause's revision. Every start
  that is not installer-held (the installer's and the module's starts, which run the watchdog's
  manual start, the watchdog's own, a reboot's) restores each persisted pause at revision 1 with
  its reason (`crates/runtime-host/src/host/scheduling_pause.rs`, `restore_scheduling_pauses`);
  only an installer-held start (actingd's `--install-held`, which neither `actingctl install` nor
  the module passed, and which Workflow #434 LAB-6S4a deleted in 0.12.63) kept the pauses its
  transition restored; since 0.12.63 every start restores them so. So the module matches the reason
  and the alias across epochs, and resumes by the epoch and revision the running Runtime shows. A
  start of the module whose readiness was never seen (it timed out and was left running) or whose
  status could not be read therefore leaves no dead end.
- **No residue after a refusal.** A refusal after the module paused instances and before anything
  changed on disk (the pause step refused, the close did not complete, `place`'s instances not idle
  in time) hands its own pauses back at once by the rule above and says so: `hold` `kept: false`
  with the release's `resumed` and `left`; `kept` is false only because nothing of the operation's
  own stays paused. A hand-back that fails keeps the hold (`kept: true`), its failure a note. After
  a change on disk the hold stays as for `actingctl install` (a failed start or post-check,
  `place`'s failed checks), with `kept: true`, `next` naming the ways out below (and `undo` where it
  applies). A `release-hold` that finds no Runtime answering (or cannot read its status) refuses
  `runtime_not_running`, its hold kept: `kept: true`, `next` with `up` when none runs.
- **A hand-back the Runtime answers by holding the instance again** (Workflow #453 S3, since
  0.12.60; register §1.365; the coordinator's ruling 4 on board #453). From 0.12.60 the Runtime
  answers the resume of an instance whose entry declares `start_paused` and that its start left
  unapplied (under the hold's pause) by lifting the hold's pause and holding the instance with
  `instance.added` ([scheduling-pause.md](scheduling-pause.md), "Pauses held at a first start"):
  `SchedulingResumed` with the new pause's revision and the self-check's `failure_code`
  `dispatch_paused_instance`. Every hand-back of the module's own hold (`release-hold`, a
  composite's release, the hand-back after a refusal) reads that answer: the instance is not
  among the release's `resumed` (so not in `release-hold`'s `count`, and its hand-back row says
  `resumed: false`) but in `held_again`, `[{alias, reason, revision}]` as S0 records a pause, the
  reason `instance.added` and the revision the Runtime answered: it stays paused as a newly added
  instance and waits for a person's resume. `held_again` is absent when no instance was held
  again, so every other release records and answers as before. No vocabulary word is added.
  `actingctl install release-hold` (its face, whose answer is the step's evidence) is
  unchanged and still lists such an instance as resumed, while the instance really stays paused
  with `instance.added`: the safe side.
- **Ways out of a kept hold** (fix 2). Every answer of the module whose own pauses stay says `hold`
  `kept: true` and gives in `next` only ways out that work today, none naming another party's
  hold: `release_hold`, the module's own `release-hold <reason>`; until LAB-7 deletes actingctl,
  `actingctl_release_hold`, `actingctl install release-hold --root <root> --reason <reason>` (it
  resumes the instances paused with exactly that reason, the module's own), and `actingctl_resume`,
  one `actingctl resume --state-root <state root> --instance <alias>` per instance the pause step
  paused that the running Runtime shows paused with the reason now (none when no status can be
  read), and (Workflow #421 INST-5b) `actingctl_resume_global`, `actingctl resume --state-root
  <state root>`, the core global resume, when the running Runtime shows the hold's own global
  pause (`actingctl install release-hold` leaves a global pause). The core `resume` is named as
  actingctl runs it until then. Since LAB-6S1b (0.12.58) the Lab's own `resume <instance>` (and
  `resume` for the global pause), which binds the Lab channel, is a way out too: the Runtime no
  longer refuses a resume from the Lab's envelope (lab-two-entries.md §2.4); `next` does not name
  it. **After LAB-7** deletes actingctl, what stays is the
  module's own `release-hold <reason>` and the Lab's core `resume <instance>` (on the Lab channel);
  LAB-7 drops the `actingctl_*` forms.
- **Its own running image.** The module runs from `<root>\tools\actinglab.exe`, which an update
  replaces: the image of the process that plans (matched by its process id) is renamed aside as
  `actingctl.exe` is and runs on (`running.renamed_own` in the plan); any other running
  `actinglab.exe` under the root still refuses `running_image_blocks` (an MCP server or another
  CLI run of it: close it first).
- **Guards report.** Every row of "Guards" below is evaluated as for `actingctl install` and none
  refuses: a row that fails or is not evaluated is noted with the warning `install_caveat_noted`,
  `install_caveat` `guard_failed` and the rows as `install_guards`, and the journal keeps the rows.
  A start waits for memory up to `install.start.memory_wait_ms` as before, then goes on and
  journals the row. The preconditions, the writer lock, an unfinished operation, the interface
  checks, the rollback gate and the checks of the inputs still refuse.
- **The composites** (operation kinds `config_replace` and `pack_place` are the module's own;
  `deploy` and `rollback` are an `update` and a `rollback` journal): `deploy <tag|commit>` stages
  (`--dir`, with its `release-facts.json` when present, or a kept build), plans, and with a Runtime
  running applies the composite update (stop with hold, apply, start and its checks), waits for the
  watchdog's `watchdog_alive` within `install.start.ready_timeout_ms` and releases the hold;
  `config replace <file>` checks the file with the live build's check-config (`--install-root`)
  before anything changes, then stops with hold, stages it as `actingd.config.json.new-<ms>.part`,
  swaps it in with the replaced file kept as `actingd.config.json.prev-<ms>`, starts, waits for
  `alive` and releases (the file's content is never printed, only its SHA-256); `config restore
  <prev>` does the same with a kept `.prev-<ms>`; with no Runtime answering both swap the file
  alone, and (Workflow #453 S3, since 0.12.60; register §1.365) only after the stopped rule,
  under the writer lock and before the operation opens: an `actingcommand-actingd.exe` under the
  root refuses `runtime_running` (`path`, `pid`), the state root's owner lock held
  `owner_lock_held` (`path`), a probe that cannot decide `processes_unreadable`, nothing swapped
  (before, one status read decided, and a Runtime that was starting, which answers none yet,
  could have its file swapped under it), and the `resume` and `undo` of such an operation apply
  the rule again first ("Instances, the first slice", "While the Runtime is stopped");
  `place <bundle.zip>` checks the `package` edge
  first (of the zip's `interfaces.json` it reads `schema_version`, which must be
  `actingcommand.component-interfaces.v1`, and `speaks`, where, by the parse rules of
  [component-interfaces.md](component-interfaces.md), every value under any name must be a pair of
  integers `[min, max]` with `min <= max`, a malformed one making the declaration unreadable, and
  `package` must be named; it takes `speaks.package`. Other names are otherwise neither required
  nor checked, and other members such as `validated_with` are not read, so a bundle that declares
  `package` alone, as a resource repository's release does, is read (Workflow #451 PLACE-PKG,
  since 0.12.59; before it `place` asked a bundle for all six v1 interfaces, the rule of a
  Runtime's declaration, and refused every released bundle); a missing, unreadable or malformed
  file, a malformed value under any name of `speaks` included, or a declared maximum
  outside the running installer's `package` range, is `interface_edge` with the path of the file
  and the game), then plans with the contract crate's readers (the layout: the root files,
  `packs/<listed id>/…`, `tables/<listed id>.json`, from `package` 10 the notices directory
  `notices/` under the rules of [package-reference.md](package-reference.md), "Notices", and,
  from 0.12.59 (Workflow #451 LOAD-P), the loading templates directory `loading/<file>`, exactly
  the templates the zip's own `loading.json` declares, read with `parse_standard_pack_loading`:
  a `loading/` member the file does not declare, a deeper path and any `loading/` member of a zip
  without `loading.json` are refused `pack_invalid` naming the member, a declared template the zip
  lacks `pack_invalid` caused by `standard_pack_root_invalid` `loading_template_invalid` (`path`
  the missing member below the zip, the pointer of the target that reads it), a problem of the
  file itself as a root file's, and a pack named `loading` in any ASCII case beside the directory
  `loading_pack_name` (`path` `<zip>\bundle.json`, `package_id`), all before the hold; an installer
  before it refuses such a zip at its layout check, naming the first `loading/` member, before
  anything is paused or written), then, while a Runtime runs (without one: "`place` while the
  Runtime is stopped", below), holds
  the instances bound to the bundle's game (the game comes from the bundle), waits until they are
  idle, places each item `.part` → `.old-<ms>` → in place (step `pack_item`), checks the bundle
  again, runs `<root>\tools\actinglab.exe standard-pack check` on the placed game root and checks
  the composition ("The composition after a placement", below), releases and reads the schedule;
  the notices directory is one item (kind `notices`, Workflow
  #444 NOTICES-2), placed first and, when the zip no longer carries it, set aside as a root file
  is; the loading templates directory is one item (kind `loading`, `<game root>\loading`, Workflow
  #451 LOAD-P), placed after the packs and before the root files, so the templates are in place
  before the `loading.json` that declares them, and, when the zip no longer carries it, set aside
  as `loading.old-<ms>` as `loading.json` is (not when a pack of the bundle is named `loading`);
  from 0.12.60 (Workflow #455 G-R1, `package` 12) the goal declaration `goals.json` is a root file
  of the layout, placed last among the root files and, when the zip no longer carries it, set
  aside as `goals.json.old-<ms>` as the other root files the readers open are, and a zip that
  carries it declares a `package` maximum of 12 at least (`pack_invalid`, `goal_undeclared` at
  `<zip>\interfaces.json`, in the plan, as `notice_undeclared`); from 0.12.61 (Workflow #463 V1
  fix round 1, `package` 14) a zip whose packs' `control.json` declare `variables` declares a
  `package` maximum of 14 at least (`pack_invalid`, `variable_undeclared` at
  `<zip>\interfaces.json` with the first such pack's `package_id`, in the plan), and (fix round 3,
  `goals.json` version 2, the only version this build reads) a zip that carries `goals.json`
  declares 14 at least (`goal_undeclared`, as before at 12); from 0.12.68 (Workflow #477 PC-1,
  [pack-catalog.md](pack-catalog.md); fix round 1: a zip that carries it declares `package` 16,
  which an installer of 0.12.67 or earlier refuses at its `package` edge, and this `place` asks no
  revision of it) the package declaration `pack-catalog.json` is the last root file of the layout, placed with
  its SHA-256 as its item's digest and, when the zip no longer carries it, set aside as
  `pack-catalog.json.old-<ms>` as the other root files are; a zip whose `pack-catalog.json` is
  wrong in its business part against the zip's `bundle.json` and `schedule.json` is refused
  `pack_invalid` caused by `standard_pack_root_invalid` (the file's error at its pointer), in the
  plan, before the hold; `ui.json`, which `pack-catalog.json` replaced, is no root file of the
  layout any more: a zip that carries it is refused as a member no layout knows, and every
  placement sets an installed one aside as `ui.json.old-<ms>`;
  `alive [--wait]`
  is the watchdog's status of the root; `instance add <alias> …` (Workflow #421 INST-5, first
  slice; "Configuration, instances, packs and a fresh root", "Instances, the first slice") composes
  the candidate and commits it as `config replace` does, with the new instance paused after the
  start (step `pause_added`) before the release, and its hold's global pause kept across the
  restart until that release (INST-5b); with no Runtime answering (Workflow #453 S3) after the
  stopped rule, its new entry declaring `start_paused`, by the swap alone. `--detach` runs the
  module's own command again through the WMI start with the same initiator. The download stays
  outside.
- **The composition after a placement** (Workflow #453 S5, since 0.12.59; register §1.345; the
  coordinator's rulings of 11-13 on board #453, not the owner's words). The journal of `place`
  records, as the input `bound`, the instances the running Runtime's status binds to the bundle's
  game, paused or not (those its hold is chosen from). The row `composition` of its `post_check`
  reads the catalog the Runtime's latest evaluation ran under. When the game's `schedule.json`
  changes (placed, replaced or set aside) and `bound` is not empty, it compares rules digests
  (Workflow #453 S5b, since 0.12.73; register §1.474; the coordinator's rulings of 11-18 on board
  #453, not the owner's words). A catalog's hash carries the catalog version, the ledger's head
  when the workbench composes, so it cannot be had beforehand; the rules digest, by which the
  workbench decides whether a check activates a new generation (it stages one only when the digest
  changed), does not move with the version.
  - *Expected.* The checks run the live build's check-config of the live configuration with
    `--install-root <root>`, as without a Runtime, for the expected digest alone: its result is
    no row of these checks (fix round 1: a note of check-config that names the placed game and
    that the running Runtime goes on with, such as a table's column past its date by the system
    clock or a package's declaration that cannot be read, fails nothing here; the rows stay
    `rerun`, `standard_pack_check` and `composition`). Its preview
    `standard_pack_schedule.rules_digest` is the expected digest: the composition of the
    installation as placed by the same composer over the same inputs as the workbench's (every
    game of the configured instances, the daemon's own reading of the packages and the
    subsystems), but the catalog version, which the digest does not cover
    ([actingd-check-config.md](actingd-check-config.md)).
  - *The Runtime's.* The rules digest of the generation its latest evaluation ran under: the
    catalog it answers names a directory of its catalog store below the state root
    (`policy\catalogs\generations\<hex>`), published once under that hash and never written
    again, whose composition record `standard-packs.json` holds `rules_digest` (none for a
    generation the workbench did not compose). The rescue module reads the daemon's own layout:
    both take it from the contract crate's constants (`CATALOG_STORE_PATH`,
    `CATALOG_GENERATIONS_DIRECTORY`, `STANDARD_PACK_COMPOSITION_RECORD_FILE` and its schema). A
    record not found where the row looks gives no digest, so a layout that moved on one side only
    could make the row `fail` (the wait ends with no digest read) or `not_evaluated` (a record it
    cannot read), never a false `pass`.
  - *The outcomes.* `expected` is `unchanged` when the expected digest is the one of the
    generation read before the hold, else `new`. The row passes once the Runtime's digest is the
    expected one, reading again every 500 ms within `standard_packs.check_interval_ms` plus
    `install.hot.idle_wait_ms`: `pass` with `catalog_hash`, `catalog_before`, `expected` and
    `rules_digest`, and with `expected` `unchanged` also `schedule_changed_rules_unchanged`
    `true`, the file changed and the composition did not (a `schedule.json` whose only new rules
    name held packages, which are not composed), at the first read; `fail` when the wait ends
    first, with `catalog_hash` (the last read), `waited_ms`, `expected` and the three digests
    `rules_digest_before` (the generation read before the hold), `rules_digest_expected` and
    `rules_digest` (the last read). A catalog or a composition record that cannot be read is
    `not_evaluated` with the failure.
  - *No expected digest* (fix round 1; the coordinator's ruling of 11-18 on board #453). When
    check-config gives no digest the row says why, with `catalog_before`, `expected` `unknown`,
    `rules_digest_expected` `null` and the cause: `check_config`, the code of check-config's
    outcome, and `problems`, its preview's `standard_pack_schedule.problems` as they are (`null`
    without a preview), or, when check-config did not answer, `failure`
    (`check_config_unanswered` of the placement, with the child's `pid` and the deadline's key, or
    its cause). It **fails** only when the placed game is the cause: check-config's outcome
    names the game (its value `game`), or its preview's composition is refused and its problems
    name the game, each problem by its value `game`, judged as the row `check_config` judges them
    without a Runtime: a problem naming the game (then also in `composition_problems`); a problem
    naming no game, the composition refused as a whole, which composes the placed game too
    (`composition_refused`); a `rule_id_conflict` recorded on another game, which cannot tell
    whether the placed bundle caused it (`collisions`, `collision_cause` `not_told`). Anything
    else is **`not_evaluated`**, not a failed row, as a catalog that cannot be read is: another
    game's `schedule.json` refused (the preview as a whole then composes nothing, while the
    workbench falls back for that game alone and goes on), a check-config refused for a reason
    that names another game or none, one that did not answer, a configuration without `policy`.
    The operation then ends as its other rows say: with them passing, done, its hold released.

  With `bound` empty it passes at its first read, `expected` `unchanged` and `bound` `[]`: the
  workbench reads only the games of configured instances (a game root no instance binds is never
  read), so no new composition comes. When the file does not change it passes at its first read,
  `expected` `unchanged`, as before. A journal without `bound` (an operation an earlier build
  began, resumed by this one) keeps the wait for a catalog other than the one read before the
  hold, reading again every 500 ms within the same time: `pass` with `expected` `new`, else
  `fail` with `waited_ms`. A catalog that cannot be read is `not_evaluated`, as before.
- **`place` while the Runtime is stopped** (Workflow #453 S6, since 0.12.59; register §1.345; the
  coordinator's rulings of 11-13 on board #453, not the owner's words; fix 1 after the
  independent review). With no Runtime answering, `place` no longer refuses
  `runtime_not_running`. The status read after its plan chooses this path: it answers "no
  Runtime" only while the state root's owner lock is not held, and a held lock whose Runtime does
  not answer (one starting, or one that does not answer) is `processes_unreadable` at that read,
  caused by the failed read, before anything is written. After its plan, which holds no running
  state (the same plan and `plan_id` as with a Runtime; `--plan` answers it as before), it takes
  the writer lock, refuses an unfinished operation, and applies the stopped rule the update
  applies after its close, before any file changes, and `install start` before it spawns (the
  update plan refuses by the same two tests): the state root's owner lock held refuses
  `owner_lock_held` (`path`), an `actingcommand-actingd.exe` running under the root
  `runtime_running` (`path`, `pid`), a probe that cannot decide `processes_unreadable`; each
  before anything is written, judged under the lock, so no watchdog tick starts a Runtime in
  between. After the status read, `owner_lock_held` here is a Runtime that took the lock since
  that read, and `runtime_running` an actingd under the root that holds no lock yet. It then
  opens one `pack_place` operation whose inputs say
  `cold: true` and `bound`, the instances the configuration binds to the bundle's game (the
  `instance_id` of each row of `policy.facts.instances` whose `game_id` is the game; none without
  a `policy`), and no `hold`, `known_pauses` or `instances`, without S0 or guards, with one
  `pack_item` step per item that changes and one `post_check`: no `pause`, no `release_hold`, and
  no item waits for idle instances (there is no Runtime to ask). Each item is placed as with a
  Runtime: `<dst>.new-<ms>.part`, the item in place renamed `<dst>.old-<ms>` and kept, the part
  renamed in, its content checked; nothing is deleted. The checks after the placement are `rerun`
  and `standard_pack_check` as with a Runtime, `check_config`, and `composition` `not_evaluated`
  with `runtime` `stopped`.
  - **`check_config`** runs the live build's check-config of the live configuration with
    `--install-root <root>`, as `config replace` checks a file, and judges two things. First,
    check-config's own result for the whole configuration: a configuration it refuses `fail` with
    its `answer`; one it does not answer within `install.child.check_config_timeout_ms` `fail`
    with `check_config_unanswered` (the child's `pid`, the key as `config_key`). Second, the
    placed game: check-config answers a problem of a game's root and a problem of a composition
    as a note beside `check_config_passed`, never as its result. Its outcome keeps at most 32
    links within 32 KiB and joins the composition's problems last, so those are cut first; its
    `report` beside the outcome is not cut. So the row judges the game from the report (fix 2)
    and from the outcome's notes, and fails on each of these, each in its own member:
    - the game's entry of `standard_packs.games` records a problem: its root's `problem`, or a
      table's `problem` or `expired_columns` (the entry, `game_root`);
    - a problem of `standard_pack_schedule.problems` names the game (`composition_problems`);
    - a problem of `standard_pack_schedule.problems` names no game: the composition is refused
      as a whole, which fails every placement whatever binds the game (`composition_refused`);
    - a `rule_id_conflict` of `standard_pack_schedule.problems` names another game: a collision
      across the games in use is recorded on the game whose name sorts later and names that game
      alone, so it cannot tell whether the placed bundle caused it (`collisions`, with
      `collision_cause` `not_told`); a file's own repeated id gives the same problem and fails
      the row alike;
    - a note of the outcome names the game (`game_notes`, fix 1), which also carries the one
      problem of the game the report does not: a `tables/` check-config could not list (the
      entry's `tables` then empty);
    - the outcome says its links were cut (`notes` `cut`, with `causes_total`) and the game's
      entry lists no table, so that one problem cannot be ruled out (`game_tables`
      `not_judged`). A cut outcome whose report lists a table of the game is judged from the
      report, and still says `notes` `cut`.

    Such a problem is of that game's root or its tables, which check-config reads below
    `standard_packs.root` whatever binds the game, or of a composition, which check-config
    previews only for the games of `policy.facts.instances`. With `bound` empty it did not
    preview the game's composition: the row says `composition` `not_previewed` beside `bound`
    `[]`, and its result is then check-config's result, the game's root and its tables, a
    composition refused as a whole and a collision. The row never judges the composition the
    Runtime will make.
  - **The composition is judged at a start**, by the Runtime's scheduler workbench, for the
    games of the configured instances only: a game no instance binds is not read until an
    instance bound to it is configured. So the answer's `next` names `composition_at_start`
    (`up`) when an instance is bound to the game (`bound` not empty), and
    `composition_at_start_once_bound` (`up`) when none is (fix 2): that game's composition is
    first judged at the first start after an instance is bound to it, not at the next start.
    A bundle that does not compose does not stop that start: the start reads and records
    every game root, a broken one never stopping it. At its check the workbench records a
    `schedule.json` the composer refuses once as `standard_pack_root_invalid`, and that game
    keeps the rules of its last good file in the catalog the Runtime had, or has no rules when it
    never had one (on a new root, none), the other games composed as they are; a broken game root
    (its `bundle.json` or another root file) keeps the catalog the Runtime had as it is until it
    is fixed, and on a new root, with no catalog yet, the Runtime runs with none.
  - **A failed row** ends the command `post_check_failed` (`count`, `game`), the items placed and
    the operation ended, its `post_check` step closed `done` with what it read (`game`, `checks`,
    `failed`) in the write that records the result, as Workflow #454 closes the post-start
    checks (from fix 1 also with a Runtime; before, the step stayed `started` with no evidence);
    no hold is kept, and `next` names `undo <operation>`, which reverses the items as for a
    placement with a Runtime (each placed item set aside as `.rolledback-<ms2>`, each kept one put
    back).
  - **Its `undo` and its `resume`** (an operation whose input `cold` is `true`) take the writer
    lock and then apply the same stopped rule before anything is moved, refusing as the placement
    does, as `install undo` or `install resume` (`owner_lock_held`, `runtime_running`,
    `processes_unreadable`); no status read comes first there, so a running Runtime is
    `owner_lock_held`. The `undo` and the `resume` of a placement begun with a Runtime are
    unchanged. The `undo` of a placement, with a Runtime or without one, asks for no
    authorization (Workflow #463 S3-2, since 0.12.64, does not change it): it restores the content
    the placement replaced, so a package the placement made leave scheduling is scheduled again,
    and no record says under which authorization. Since 0.12.69 (Workflow #463 S3-18) it says so
    before, in its answer and on record ("The packages the undo of a placement makes enter or
    leave scheduling", below), and still asks for none.

  The answer's steps are `plan` and `place`, with no `schedule`. While the operation is unfinished
  the watchdog stands aside (`pack_place` is off row 7a″'s pass list), so no Runtime starts on a
  half-placed game root; the next start reads the placed game root.
- **The packages a placement makes enter or leave scheduling** (Workflow #463 S3-1, since
  0.12.64; register §1.408; the coordinator's rulings on board #463, not the owner's words). A
  placement is the act that enables: once a game root is placed, the workbench composes it and
  dispatches every package a rule names and no hold keeps back. So the plan of `place` (its
  `--plan` answer and the answer's step `plan`) says which packages enter or leave scheduling,
  in its member `schedule_changes`, before anything is placed. It is read only: the plan
  document, the plan id, the items and every check are as before, and nothing a placement writes
  reads it.
  - **The classification.** The packages of the bundle's game are classified on the installed
    game root (`<standard_packs.root>\<game>` as it is: `bundle.json`, `pages.json`,
    `interrupts.json`, `navigation.json` and `display-layout.json` read as every reader reads
    them, the bundle's `maintenance` entries, and `schedule.json`; since fix round 2 each file
    read as readiness reads a game root's file, at most 32 MiB) and on the zip's root (the same
    files of the zip), each by the contract crate's own `standard_pack_maintenance_packages` and
    `classify_standard_pack_packages`, the functions readiness calls. A side is **composed** when
    its root holds a `schedule.json` that the contract crate's reader and the composer's own
    reference check (`validate_standard_pack_schedule_references`: every rule's and every held
    entry's package is a package of the root, every `server` a server of one) both take: the
    workbench leaves a game root without `schedule.json` out of its composition, and a game whose
    file its composer refuses keeps the rules of the active generation, which the plan does not
    read ([standard-pack-schedule.md](standard-pack-schedule.md), "Unload", "Last good rules").
  - **A package's part** on one side, `{state, reason?, reference?, roles?}`: `state` is
    `scheduled` (named by a rule and not held), `held` (with the entry's `reason` and, when
    written, its `reference`), `maintenance_role` (a role only), `unclassified` (in no set),
    `absent` (not listed by that root's `bundle.json`: a game's first root, a pack the zip no
    longer carries) or `unread` (below); on a root without `schedule.json` it is what the root
    files say (`maintenance_role` or `unclassified`). `roles`, present only when there is one,
    names the supporting parts the root files give it, in readiness's words (the vocabulary
    `readiness_role`, [standard-pack-schedule.md](standard-pack-schedule.md), "Readiness"), each
    found by calling `standard_pack_maintenance_packages` on the entries of one use, or on one
    list, alone, and (since fix round 3) the plan's own word `prerequisite`, which is not one of
    readiness's words: a layer of a prerequisite chain (below). The plan lists them in this
    order: `startup`, `return_home`, `recognition` (the uses of its `bundle.json` `maintenance`
    entry), `features` (`pages.json`), `edge`, `chain` (the edges and the recovery chain of
    `navigation.json`), `handler`, `then_run` (the handlers and the `then.run` packages of
    `interrupts.json`), `prerequisite`. A package held or scheduled that has a role is `held` or
    `scheduled` with its `roles`; a package used only as a prerequisite keeps the state the
    classification gives it (`unclassified`, say) with the role `prerequisite`.
  - **The prerequisite chains** (since fix round 3). A pack's `control.json` may name, in
    `prerequisite_package_id`, the package the host runs as a layer of the pack's runs
    ([linear-steps.md](linear-steps.md), "Prerequisite packages"; the Lab's recorder writes it
    from `requires`). On each side, from each package that is scheduled or holds one of
    readiness's words, the plan follows that member as the host resolves it when a run is
    prepared: the member of the `control.json` in the package's directory that the side's
    registry (its `bundle.json`) gives, the package it names looked up in the same registry,
    then that package's own, at most three layers (the execution kernel's
    `MAX_PREREQUISITE_DEPTH`; the host refuses a fourth). Each layer reached holds the role
    `prerequisite`; a package that holds only that role starts no chain of its own. A layer the
    side's registry does not list, or one already in the chain, ends the chain and counts
    nothing: the host refuses that run at preparation, which is another check's refusal, and the
    plan says nothing of it; so does a `control.json` that is no JSON object or whose member is no
    string (the package's admission refuses it). In that case, and with a fourth layer, the host
    refuses the whole run while the plan still counts the package that starts the chain and the
    layers before its end as used, so the lists hold more than what runs, as for a root the
    composer would refuse (below). The installed side's `control.json` is read as
    its root files are (at most 32 MiB), the zip's from its member
    `packs/<directory>/control.json`. The rescue module does not link the host or the execution
    kernel, and the contract crate has no reader of the member, so the plan reads it itself, with
    the zip reader and the bounded file read it uses for the root files.
  - **`packages`**: each package whose part or pack content changes, or that one of the lists
    names, by package id, `{package_id, before, after, content?}`; `content` is the state of its
    pack item (`new`, `replace`, `unchanged`), absent for a pack the zip no longer carries (its
    folder stays in place; only `bundle.json` stops listing it).
  - **`enters`** lists the packages that, by the schedule, the root files and the prerequisite
    chains of the two sides, will be dispatched or used after the placement and were not before:
    the sorted package ids `scheduled` after and not before, or holding a role after that they
    did not hold before (`prerequisite` included), whichever root file or chain the role comes
    from. Only a composed side counts: on a side that is not composed, and on an installed side
    that could not be read, no package counts as scheduled or as holding a role, for `enters`,
    `leaves` and `content_only` alike. So a lifted hold enters; a package that becomes the
    prerequisite of a scheduled one enters, and the prerequisite of a held one does not; on a
    game's first root, and over an installed root without `schedule.json`, every package the zip
    schedules or gives a role enters; a zip without `schedule.json` makes nothing enter. A
    package of a server to which no instance is bound is listed like any other: binding an
    instance later needs no placement, so the authorization covers it now. The plan does not run
    the composer: a root the composer would refuse for another reason than the reader's and the
    reference check's (in its `read_game`: a member whose subsystem `requires` does not name, a
    subsystem this Runtime does not name, a member only the composer writes, a `limited` that is
    not `true` or sits on a maintenance rule, a realization, limited or fixed-value mark on a
    rule of a maintenance-role package, a timeline event that invalidates the application update
    fact; after it: a package another game's rules already name or an id two composed items
    share, `rule_id_conflict`, and the compile of the joined catalog) is counted as composed, so
    the lists then say more than will happen; for a game bound to an instance the placement's own
    post check fails in that case (the composer leaves out a game no instance is bound to, and the
    post check does not judge it), and a placement made while a Runtime runs keeps its hold. **`leaves`**: `scheduled` before
    and not after, a role it held before and does not after (so every package used before leaves
    when the zip carries no `schedule.json`, and a prerequisite leaves with the last package
    whose chain reached it), or no longer carried by the zip (also one never scheduled).
    **`content_only`**: a package the scheduler uses before and after in the same way and whose
    pack is replaced (one used only as a prerequisite too). A package may enter and leave at once
    (it loses its rule and gains a role). The plan of a bundle that changes nothing lists nothing
    in any of the four.
  - **`unread`**, present only when a side could not be read: `[{root, failure}]`, `root`
    `installed` or `bundle`, the failure as a warning (`{outcome}`). A file of the installed root
    that cannot be read, or one larger than the bound (`foreign_os_error`; `io_kind`
    `file_too_large` for the bound; since fix round 3 a pack's `control.json` too, one the pack's
    directory does not hold naming no prerequisite), or root files the reader refuses
    (`standard_pack_root_invalid` at the file, its problem and pointer) leave every package of
    that side `unread` without roles; a `schedule.json` that the reader or the composer's
    reference check refuses
    (`standard_pack_root_invalid` at that file, its problem and pointer; the plan itself checks
    neither side's) leaves each package of that side `unread` with its roles. The zip's side is
    unread only through its `schedule.json`: the plan refuses a zip whose other root files the
    reader refuses. An unread installed side counts nothing before, so every package scheduled
    or holding a role after enters, and `leaves` and `content_only` are not complete. With the
    zip's side unread the answer carries `packages` and `unread` and none of the three lists:
    what the scheduler uses after the placement is not known. Nothing is refused for it.
- **The authorization a placement names** (Workflow #463 S3-2, since 0.12.64; register §1.409;
  the coordinator's rulings on board #463, not the owner's words). The owner's rule is that
  nothing enters normal scheduling without her explicit authorization, and the placement is the
  act that enables. So a placement whose plan changes something names the authorization it acts
  under and, one by one, the packages it enables; `place --plan` and the placement of a plan
  that changes nothing (every item `unchanged`) need neither and read neither.
  - **The parameters.** `--authorization <reference>`: the reference of the authorization (a
    board comment's address, say), non-blank, at most 1024 bytes, no control character.
    `--enables <package_id>`, repeated: every package of the plan's `enters`, once each (since
    S3-1's fix round 3 also a package used only as a layer of the prerequisite chain of a
    package scheduled or holding one of readiness's words), and absent when nothing enters. There is no default, no variable and no
    switch that places without them.
  - **The checks**, after the plan and before the hold, any pause and any write, once for both
    paths (with a Runtime and while it is stopped), in this order, each refusal with `plan_id`
    and `game`: no `--authorization`, or a blank one, is `install_command_refused`
    `place_authorization_missing`; one longer than 1024 bytes or holding a control character
    `actingctl_usage_invalid` (`unusable`, `--authorization`); a plan that answers no `enters`
    (the zip's side unread: its `schedule.json` refused by the reader or the composer's reference
    check, so the plan carries `packages` and `unread` and none of the three lists)
    `place_enables_unknown`, the plan's `schedule_changes` beside the outcome, whose `unread` names
    the refusal (`standard_pack_root_invalid` at the file): the packages the placement would
    enable cannot be named. The plan alone decides this; the check reads only its answer. The
    zip's other root files are refused `pack_invalid` in the plan already; an installed game root
    that cannot be read refuses nothing, since a placement is how a broken root is repaired: it
    counts nothing before, so its `enters` are every package scheduled or holding a role after.
    Last, `--enables`, sorted, not exactly the plan's `enters` (a name missing, one too many, one
    given twice, or any when nothing enters) is `place_enables_mismatch`, the answer's `expected`
    (`enters`) and `given` (`--enables`), each sorted, beside the outcome. A caller without a
    reference learns that first.
  - **The record.** The `pack_place` journal's `inputs` gain `authorization` (the reference as
    given) and `enabled` (the sorted list of the packages the placement enables, `[]` when none),
    with a Runtime and without one; `op status <op>` shows them with the journal. A journal
    an earlier build began has neither member: it loads and is resumed and undone as before, and
    its authorization reads as not recorded, never as an empty one.
  - **What the tool does not do.** It records an authorization and grants none: it does not judge
    the reference, and never supplies one. `--enables` is the caller's statement that it knows
    which packages it is enabling; the plan's `enters` is what the tool holds it to. A plan that
    lists `enters` is not an authorization, and neither is a passed check.
- **The packages the undo of a placement makes enter or leave scheduling** (Workflow #463 S3-18,
  since 0.12.69; register §1.452; the coordinator's rulings on board #463, not the owner's
  words). The `undo` of a placement restores what the placement replaced, so a package the
  placement made leave scheduling (its rule removed, a hold added, a pack no longer carried) enters
  again, and one the placement made enter leaves. The owner's rule is that no tool lets a package
  enter scheduling by itself without the person seeing it; so the `undo` of a `pack_place`
  operation says what it changes of scheduling, before, in its answer and on record.
  - **The computation.** Before its first step (after the refusals it gives before it writes,
    and, for a placement begun while no Runtime ran, under the writer lock and after the stopped
    rule), the undo computes `schedule_changes` from the game root as it is to the game root as the
    undo leaves it, by the placement plan's own judgement ("The packages a placement makes enter
    or leave scheduling", above): the same classification of the same files, read as the plan
    reads the installed game root, the side after the undo reading at each place the undo changes
    what it leaves there, the kept `.old-<ms>` item that it puts back or nothing where it sets the
    placed item aside and puts nothing back, by the rules of its `pack_item` steps it reverses (the
    steps started or done; a place that holds neither the placed item nor nothing stays as it
    is). Nothing is reversed or moved for it: the side after the undo is read where its files are
    now. The game and its game root are those the operation's `post_check` step records.
  - **What it answers.** `schedule_changes` has the plan's members with the plan's meaning:
    `packages` (`before` is the game root as it is, `after` as the undo leaves it; `content` the
    state of what the undo does to the pack: `retire` for a pack it sets aside with nothing put
    back, `replace` for a kept pack put back over the placed one, `new` for a kept pack put back
    where nothing is), `enters`, `leaves`, `content_only`, and `unread`, whose `root` is
    `installed` (the game root as it is) or `undone` (as the undo leaves it). With the side after
    the undo unread there is no list: `packages` and `unread` only, as in the plan. So the undo of
    a placement that added two packs, each named by a rule, lists both in `leaves` and nothing in
    `enters`; the undo of one that removed a pack's rule or held it lists that pack in `enters`;
    the undo of one that only replaced a pack's content lists it in `content_only` alone.
  - **The warnings**, each a note link of the answer's outcome (`outcome.causes`, relation
    `note`; `install_caveat_noted` of `install_undo` with `game`), on the command line and through
    MCP alike, exit 0, never a refusal: `undo_enters_scheduling` when `enters` is not empty, its
    `names` those packages, sorted (the undo lets them enter scheduling, or take a supporting
    role, again); `undo_scheduling_unknown` when the side after the undo could not be read, so no
    list is answered, the failure of that side (`unread`'s `undone` entry) as its cause: what the
    undo changes of scheduling is not known. One cause per word. The undo goes on as before in
    both cases.
  - **Before: `--plan`.** The rescue module's `undo <op> --plan` (the row's switch `plan`), for a
    `pack_place` operation only (another kind is refused `actingctl_usage_invalid`, `unusable`,
    `--plan`, once its journal is loaded and before anything else of its undo is judged;
    `actingctl install undo` does not take the flag and refuses it as unknown, its face
    unchanged), judges what the undo judges before its
    first write, the same refusals in the same order (for a placement begun while no Runtime ran,
    its stopped rule), but takes no writer lock and writes nothing, as no plan form does; then
    answers `install_command_done` of `install_undo` (`install_operation_id`,
    `installation_changed` `false`, the warnings above as notes) with `operation_id`, `reverses`
    (the steps the undo would reverse, in its order, each `{n, kind, target}`) and
    `schedule_changes`: the lists the undo, run next on the same root, answers.
  - **In the answer** of the undo done: `schedule_changes` as computed before its first step,
    beside `operation_id` and `kept`, and the warnings above as notes of its
    `install_command_done`.
  - **An undo run again after it stopped** (fix round 1). Once the undo record is written, the
    game root is half undone (the steps are reversed from the last, so a root file such as
    `schedule.json` is put back before the packs), and computed again the lists would no longer
    name what the undo lets enter. So nothing is computed again: an undo that goes on after it
    stopped answers the `schedule_changes` its undo record holds, `{enters, leaves}` (both `null`
    when they were not known), as `op status` shows it, and its warnings from it:
    `undo_enters_scheduling` naming the recorded `enters` when not empty, `undo_scheduling_unknown`
    when they are `null`, without a cause (the record keeps no failure; the first run's answer
    carried it). Its plan form answers the same, and so does `op resume` of an undo that ended its
    steps but whose `current.json` was not retired: its `schedule_changes` beside the outcome, the
    warnings as notes of the undo's own `install_command_done`, which `op resume` links `related`.
    An undo an earlier build began, whose record holds no `schedule_changes`, answers none and no
    warning. Only the undo's first run, and the plan form before it, compute.
  - **On record.** The journal's undo record gains `schedule_changes` `{enters, leaves}`, each the
    sorted list, both `null` when they were not known, written with the record when the undo
    starts (an undo that goes on after it stopped keeps the record it has, and answers it);
    `op status <op>` shows it with the journal. A record without it (the undo of another kind, or
    one an earlier build wrote) loads and is shown without it.
  - **What it does not do.** It asks for no authorization and records none: whether the undo of a
    placement must name an authorization, as the placement does, is the owner's to rule and is
    open. What the undo reverses, its order, its refusals, its writer lock and stopped rule, and
    the undo of every other kind of operation are unchanged.
- **What the placements enabled** (Workflow #463 S3-15a, since 0.12.69; register §1.451; the
  coordinator's rulings, not the owner's words). The one public read the rescue crate offers
  another module, beside its rows (`crate::rescue::placements_enabling`, used by `actinglab
  standard-pack status --installation`, [standard-pack-schedule.md](standard-pack-schedule.md),
  "The installation's part"): for an installation located as the module's rows locate one and a
  game root, first whether the game root is the installed one of its game (the configuration's
  `standard_packs.root`, a relative one against the root, and the game below it, as `place` finds
  one), else nothing more is read and the answer says so (`OtherRoot`, with the installed game root
  where the configuration names one); then every journal by the journal's own reader, which stays
  private and unchanged, and per package id the newest `pack_place` operation of that game that
  finished (`finished_at_unix_ms`) with every `pack_item` step `done`, was not undone (an `undo`
  record, begun or ended, excludes it) and not rolled back (a `rollback` record excludes it), and
  names the package in its input `enabled`: its `authorization` as given, its `initiator`, its
  `finished_at_unix_ms` and its operation id. A placement that ended before it placed an item (its
  instances not idle in time) has its items `planned` and does not count; one whose check after
  the placement failed placed them and counts. A journal an earlier build began names neither input
  and enables nothing here. A journal that cannot be read is named (`operations_unread`) and hides
  none of the others; the answer also counts the journals read. It writes nothing, starts nothing
  and asks no Runtime; a root it cannot resolve is `install_command_refused` `state_invalid`, a
  configuration it cannot read `config_unreadable`, an operations folder it cannot list that
  listing's failure, each of the operation `install_status`. No journal, no write and no row of the
  module changes. Fix round 1 (register §1.451, "Fix round 1"): the read is two calls, so that a
  caller can start other work between them (`standard-pack status` starts its `actingd suspended`
  there): `placements_enabling` checks the game root only, comparing it with the installed one as
  written, then resolved (by `diagnose`'s one comparison of two directories, `lab-ops`
  `diagnose::same_directory`), and answers `OtherRoot` or `Installed`; the journals are read by
  `Installed`'s `placements()`, which answers the listing's failure.
- **`vision manifest --provider cpu|directml` and `vision check`** (Workflow #453 S7, since
  0.12.60; register §1.355; [vision-model-folders.md](vision-model-folders.md), "The manifest";
  the coordinator's rulings on board #453, not the owner's words). Two read-only rows, `runtime`
  `offline`, `writes` false; no `actingctl install` command. `vision manifest` runs the live
  build's `actingd vision manifest --provider <p>` and answers its manifest as `report`
  (`install_command_done`, operation `install_vision_manifest`). `vision check` reads the root's
  configuration as every row does; then (fix 1, in this order), for each game root below the
  configured `standard_packs.root` (a relative one against the configuration's folder; each
  folder there that is no junction or link, whose name is UTF-8 and that holds `bundle.json`, by
  name; none when the root names none or it does not exist), the Tools' `<root>\tools\actinglab.exe
  --json standard-pack check --root <game root>`, as `place` runs it; then the live build's
  `actingd vision check --config <the root's configuration> --install-root <root>` with
  `--model <model_ref>` for each model those checks name, so that actingd computes the identity
  of their folders too. It joins the checks' `models`: `required_models`, each `{model_ref,
  model_sha256, games: [{game, packages}], present, identity_matches}`, by `model_ref` then
  `model_sha256`, judged against the folders actingd listed (fix 2: a name actingd lists
  `absent` is no folder, so not `present`). The answer holds actingd's
  `report`, `standard_packs {root, games, skipped}` and `required_models`; it is
  `install_command_done` (operation `install_vision_check`) when every manifest file is `ok` and
  every model matches, else `install_caveat_noted` with `install_caveat` `vision_incomplete` and
  `count`. **Nothing below the standard-pack root is skipped silently** (fix 1): every entry
  there that is not checked is named in `skipped`, `{name, why}` by name, `why` one of `link`
  (a junction or a symbolic link: the Runtime lists no game root through one, so the check does
  not check it either), `name_not_utf8` (a folder whose name is not UTF-8, named lossily: the
  Runtime lists none), `no_bundle` (a folder without `bundle.json`: it holds no pack a Runtime
  runs) and `not_a_folder` (a file). A game root whose `standard-pack check` fails keeps
  refusing the row, and the refusal names that root (`pack_invalid` with `game` and `path`).
  Every child runs within a bound of what it does and is ended there, never left running, as the
  row `emulators` ends its child: `actingd vision manifest` 30 s (it reads only what is compiled
  into it); each `standard-pack check` 300 s (it admits every package of one game root, each
  within 60 s); `actingd vision check` 160 s (fix 1: it hashes at most 512 MiB in all, the
  manifest's files included, each folder's file at most 256 MiB; 512 MiB at 4 MiB/s is 128 s,
  and 32 s is the process's start and its listing). The row `vision check` also has a limit in
  all (fix 1): its children's bounds add up to 160 s and 300 s per game root, and the row runs at
  most 900 s, the default of `install.child.check_config_timeout_ms`, the longest `place` waits
  for one such check: a row that only reads does not keep a person or an installer waiting longer
  in all than the installer waits for one check of one game root. Each child runs within its own
  bound or what is left of the 900 s, whichever is less. A refusal for a child ended at a bound
  has `timeout_ms` and no cause: the child's own bound, or 900000 when the row's limit ran out
  first; a child that would start with nothing of the limit left is not started and refused the
  same way. Refusals: `vision_report_unanswered` (fix 1) when actingd gives no usable report
  (`vision manifest`: every failure of the row, which resolves no root; `vision check`: actingd
  printed no report, refused with no stage, which is every refusal but those below (its
  installation check or slot lock, `--install-root`, its command line, among them more than 64
  models the packs need or a model name no folder may have, its own registry), printed one
  without its lists, did not run, or was ended), its outcome or the child's failure as the cause;
  `vision_root_unresolved` (of these two rows only `vision check`, and `vision place` below; fix 2:
  decided by the stage actingd names, not by what its refusal is not) when actingd refused at a
  stage, which it names only where it loads the configuration (`config_load`), assembles its
  `vision` section (`config_assemble`) and resolves
  the vision root (`config_vision`): its outcome, for example `vision_root_unavailable`, as the
  cause;
  `config_unreadable` for a `standard_packs.root` that cannot be listed or a `bundle.json` whose
  kind cannot be read; `pack_invalid` (`game`, `path`) for a game root whose check gives no
  models. Nothing is written, downloaded or placed; placing the files is `vision place`'s (below).
- **`vision place --from <dir>`** (Workflow #453 S9, since 0.12.60; register §1.367;
  [vision-model-folders.md](vision-model-folders.md), "Placing a vision root from a folder"; the
  coordinator's rulings on board #453, not the owner's words). A row of this module only
  (`writes` true, `runtime` `offline`); `actingctl install` has no such command. It fetches
  nothing and names no address.
  - **Which files, and where.** The live build's `actingd vision check --config <the root's
    configuration> --install-root <root>` (as `vision check` runs it, without `--model`, within its
    160 s and ended there) resolves the configured vision root and its provider as a start does,
    and answers every file of that provider's manifest with its size, SHA-256 and state there; the
    files whose state is not `ok` (`missing`, `sha256_mismatch`, `unreadable`) are the ones to
    place. A `directml` configuration's manifest holds every file of the `cpu` one and DirectML's
    own, so a configuration that can fall back to the CPU (`gpu_fallback` `cpu`) gets what both
    need; one without a `vision` section, or of `cuda`, has no manifest. The module links no vision
    crate. A refusal of actingd at a stage is `vision_root_unresolved` (a vision root that does not
    exist yet included: actingd resolves it as a start does, so its folder must exist), any other
    failure `vision_report_unanswered`, as for `vision check`; both before anything is written.
    **The row never creates the vision root** (the coordinator's ruling on board #453): to create
    it, it would have to resolve the configured root itself, repeating actingd's resolution, or
    write before it holds the writer lock; the slice that builds a fresh root creates the folder.
    **A file in its place that cannot be read** (`unreadable`) is treated as one that differs (the
    same ruling): renamed aside under its kept name `<dst>.old-<ms>` and replaced. When that rename
    itself fails (another process holds the file), the step fails after the earlier steps wrote:
    the command ends `install_operation_interrupted` with the OS failure as its cause, and the
    operation is unfinished, so `op resume` (once the file is free) finishes it and `undo` reverses
    what it placed.
  - **`--from`** is laid out as a vision root: each file at its `vision_root_path` below it. Before
    anything is written, each file to place is read from it and compared with its pin, its size
    and then its SHA-256: one missing, of another size or another SHA-256, or unreadable refuses the
    whole command `vision_source_invalid` with its `path` (the answer's `source` names its place,
    its state and its pin; an OS failure is the cause); a `--from` that is no folder is refused the
    same way. A file below `--from` that the manifest does not name is neither read nor copied; the
    answer's `ignored` counts them. Every file of this build's manifest is pinned, its license
    files too (S7, "The manifest"); a report that names a file without a size and a SHA-256 is
    `vision_report_unanswered`, so no file is ever copied unverified.
  - **While stopped.** Then, under the writer lock, with no operation unfinished, the stopped rule
    of the update path (as `place` applies it while no Runtime answers: `owner_lock_held`
    (`path`), `runtime_running` (`path`, `pid`), `processes_unreadable`), before anything is
    written. **The files to place are taken again under the lock**, each manifest file of the
    vision root compared with its pin as actingd compares it (its size, then its SHA-256): another
    run that placed them while this one waited for the lock leaves none, and the answer is then a
    complete root's (no operation opened, so no `undo` of this run could set aside what the other
    placed); a file found wrong only then is verified below `--from` first, as above.
  - **The operation** (kind `vision_place`; off row 7a″'s pass list, so the watchdog stands aside
    while it is unfinished): its inputs `commit`, `provider`, `vision_root`, `from`, `ignored` and
    `config_sha256`; no plan, no S0, no guards; one `vision_file` step per file to place, in the
    manifest's order (the file copied from `--from` to `<dst>.new-<ms>.part` and checked, a file in
    its place renamed `<dst>.old-<ms>` and kept, the part renamed in and checked again), then
    `post_check`, the same `actingd vision check`: one row `vision_check`, passed when every
    manifest file is `ok`, every manifest model's identity matches and the vision root is the one
    placed into. Done is `install_command_done` (`install_operation_id`, `installation_changed`
    true) with `provider`, `vision_root`, `from`, `ignored`, `placed` (each step's evidence: the
    place, the file and the kept names), `checks` and `next` (`undo`). A failed row ends the
    command `post_check_failed` (`count`), its step closed with what it read, the files placed and
    `next` naming `undo`. A step that fails after a change on disk is
    `install_operation_interrupted`; a copy or a placed file that does not hold its pin is its
    cause as `vision_source_invalid` (`path` that file).
  - **A complete root** answers `install_command_done` with `installation_changed` false,
    `placed` `[]` and actingd's `report` (as actingd read the root before the lock), and opens no
    operation.
  - **`resume` and `undo`** (`op resume`, `undo`) take the writer lock and apply the stopped rule
    again before anything is moved. A crash between the two renames of a step leaves the file of
    before renamed `.old-<ms>` and the part waiting: `resume` renames the part in, `undo` renames
    the `.old-<ms>` back. `undo` sets each placed file that holds its pin aside as
    `.rolledback-<ms2>`, puts a kept `.old-<ms>` back and sets a part left behind aside as
    `.abandoned-<ms2>`. Nothing is deleted, and no file the manifest does not name and no other
    folder under `models\` is touched. `leftovers` lists no item of a vision root, as it lists
    none of a game root.
  - Until 0.12.62 it fell under the operating rule that kept the module's rows off the production
    root ("Journals across versions" below).
- **`state-root claim [--new-identity]`** (Workflow #453 S12, since 0.12.60; register §1.368;
  [state-root-installation.md](state-root-installation.md), "Handing a state root over"; the
  coordinator's rulings on board #453, not the owner's words). `writes`, `irreversible`, `runtime`
  `offline`; no `actingctl install` command. It reads the root's configuration as every row does,
  applies the update path's stopped rule (`owner_lock_held`, `runtime_running` with the image's
  `pid`, `processes_unreadable`), then runs the live build's `actingd claim-state-root --config
  <the root's configuration> --actor <initiator>` (and `--new-identity` when given) within
  `install.child.check_config_timeout_ms`, as `unlock` runs `unlock-owner`: no installer command
  writes into a state root, actingd does. A completed claim is `install_command_done` (operation
  `install_state_root_claim`, `path` the state root, `installation_changed` as actingd's
  `changed`, actingd's outcome `state_root_claimed` as its `related` link) with actingd's report
  as `report`, the initiator as `actor` and, when a record was replaced, its claim time as
  `previous {claimed_at_unix_ms}` (when it could be read) and the note `install_caveat_noted` with
  `install_caveat` `fingerprints_discontinuous` (`path` actingd's `kept_record`, the record set
  aside) whenever a record of another installation or one that could not be read was replaced,
  and (fix round 1) whenever a record was written while the state root held an old record an
  earlier hand-over kept (`path` the newest such record): the ledger's records that
  carry fingerprints were made with the salt of the configuration that wrote them, and the command
  cannot know whether this configuration's salt is the same. actingd's refusal is the answer, as
  `unlock`'s is; no report from actingd (it did not run, printed none or was left running at its
  deadline) is `install_command_refused` with `install_refusal` `state_root_claim_unanswered`
  (`path` the state root), the child's failure as the cause or `pid` and `config_key`. The row
  itself writes nothing in the root.
- **`op status --state-root-installation`, the member `state_root`** (Workflow #453 S12, since
  0.12.60; fix round 1, the coordinator's ruling after the review). **Only when the caller asks
  with the switch `state_root_installation`** (off by default, declared in the operation table,
  read by the module and never handed to `install status`) does the rescue module's `op status`
  also read the live build's check-config of the root's configuration (`--install-root <root>`)
  and answer `state_root {path, installation}`: the configured state root and its installation
  state as check-config reports it, `claimed_by_this`, `unclaimed` or `not_installed` (`null` from
  a build before 0.12.60, whose report has no such member); when check-config refuses (another
  installation's record is `state_root_installation_mismatch`, a file that cannot be read
  `state_root_installation_record_invalid`, each at stage `config_state_root`) or gives no answer,
  `installation` is `null` and `check_config` holds that outcome. **Without the switch it starts
  no process and its answer is byte for byte what it was before S12.** With the switch while
  `install status` names an unfinished operation, the answer is the status without the member and
  no process is started: the update path counts any actingd under the root as a running Runtime
  (the stopped rule, the plan's running images), and a background operation's answer tells its
  caller to follow it with `op status`, so a check-config started then could make the operation's
  stop answer `runtime_running` or its plan count one Runtime too many; the answer's own
  `unfinished` says why the member is not there. Read only; `actingctl install status` is
  unchanged.
- **Until LAB-6** a request the module sends to a running Runtime goes on the CLI channel with the
  CLI envelope, as `actingctl install`'s do.
- **Journals across versions** (the coordinator's ruling of 2026-11-10, LAB-3 fix 1; no
  `install-root` raise). An installer of
  v0.12.47 or older does not read a journal the module wrote (its `initiator`, `one_way`, the
  kinds `config_replace` and `pack_place`; since 0.12.60, Workflow #453 S9, the kind
  `vision_place` and its step `vision_file`, which a build before 0.12.60 does not read either):
  the testing phase keeps no old-version compatibility
  (Alice, 2026-10-11). `actingctl install` writes its journals byte for byte as before, and this
  build reads the older ones. **Operational rule until LAB-7** (lifted in 0.12.62): until 0.12.62
  production deployed through `actingctl install` (the coordinator's deploy kit) and kept the
  module's `deploy`, `rollback`, `place`, `config replace`, (Workflow #453 S9) `vision place` and
  (Workflow #453 S12) `state-root claim` off the production root, so that no journal an older
  installer cannot read appeared there. The owner's ruling of 2026-11-15 keeps nothing compatible
  with earlier builds, so that reason is gone: only the build a deploy replaces must read the
  module's journals, and that build (v0.12.61 for 0.12.62) reads every kind above, its
  `actingctl install` being the module's own code (`crates/lab-rescue`). Since 0.12.62 production
  deploys through the module (`release stage`, `deploy`, `op status`, run from the live build's
  kept `actinglab`), with `actingctl install` of the build it replaced as the way back
  (`install resume`, `install undo`, `install release-hold`); the rule keeps no row of the module
  off the production root any more.

## Interface checks

From [component-interfaces.md](component-interfaces.md), "Checks per operation". The parties:
**I** the running installer (its own declaration), **Rp** the live build, **R\*** the build that
will run, **B** a placed bundle (the register's declared range) or a new one (its
`interfaces.json`), **C** the configuration file, **U\*** the UI that runs after the operation
("UI edges" below).

| Operation | Edges (all must pass; each a row of `edges`) |
|---|---|
| update, rollback | `ledger`: Rp.max ∈ R\*; R\*.max > Rp.max needs `--accept-ledger-one-way` (journalled; it closes later rollbacks by the same edge). `runtime-outcomes`: R\*.max ∈ I. `install-control`: I ∩ Rp and I ∩ R\*. `runtime-client`: I ∩ Rp and I ∩ R\*. `package`: every placed B.max ∈ R\* (unknown: the warning). `actingd-config`: C passes R\*'s check-config, and I's key table knows every key C names in the sections the configuration mechanism resolves. Each build's Runtime and Tools manifests declare equal `speaks`. Since 0.12.80 (Workflow #429 RB-1): `requires`, R\*'s check-config names every subsystem the schedules a start of R\* would read require (`next_names_required`, a row only when it fails; "Subsystems the schedules require" above). |
| config commit, restore | C passes the check-config of the build in `runtime\`. |
| pack place | B.max ∈ the live Runtime's `package` and ∈ I's, checked before the layout; a bundle that carries `notices/` declares B.max ≥ 10 (`pack_invalid`, `notice_undeclared`); from 0.12.60 a bundle that carries `goals.json` declares B.max ≥ 12 (`pack_invalid`, `goal_undeclared`), from 0.12.61 ≥ 14 (`goals.json` version 2); and from 0.12.61 one whose packs declare `variables` declares B.max ≥ 14 (`pack_invalid`, `variable_undeclared`). |
| new | I ∩ R for `actingd-config` (Workflow #453 S4: `installer_meets_next`, before anything is written); R's check-config passes the first configuration (`write_config`). |

An edge row is `{interface, check, result: pass | fail | warning, parties: [{party, source,
range}]}` (`source` `manifest`, `register` or `bundle`); every failed edge names both parties,
the source of each range and the ranges.

### UI edges (Workflow #429 A.4 a, R-15a, since 0.12.30)

**U\*** is the UI that runs after the operation. Its ranges are the declaration its UI build
carries: the interface repository's declaration file, which the UI build
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

A root has no U\*: the update and rollback plans and the rollback gate's part 1 hold no U\* row.

## Safety

### Preconditions

| Command | Runtime | owner.lock | writer.lock | Other |
|---|---|---|---|---|
| `update` / `rollback apply` without `--stop` | stopped: no actingd under the root | unlocked | taken | no unfinished operation; no `.part` of another operation; no running image that would change but `actingctl.exe` and `actinglab-mcp.exe`; a running `adb.exe` never changes |
| with `--stop` (INST-2) | running (stop is step 3) | — | taken before the hold | as above after the stop |
| `config commit apply` (INST-3) | stopped, or `--while-running` | — | taken | candidate base = live |
| `pack place` (INST-4) | any (cold when stopped; `--hot` when running) | — | taken | hot: its guards |
| `start` (INST-2) | stopped | unlocked | taken (or already held) + `run.lock` | configuration passes check-config |
| `stop` (INST-2) | running | — | not taken alone; taken inside an update | its guards |
| `new` (INST-6; the rescue row, Workflow #453 S4) | — (never started) | — | created | the root and the state root absent or empty folders; the build verifies; I ∩ R for `actingd-config` |
| the rescue module's `config replace`, `config restore` (LAB-3; Workflow #453 S3) | running (stop with hold), or stopped: no actingd under the root | unlocked when stopped | taken | the file passes the live build's check-config; stopped judged under the writer lock before the operation opens |
| the rescue module's `instance add` (INST-5; Workflow #453 S3) | running (stop with hold; the new instance paused after the start), or stopped: no actingd under the root (the new entry declares `start_paused`) | unlocked when stopped | taken | stopped judged before the candidate is written and again under the writer lock; the candidate passes the live build's check-config |

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
  every other kind stands aside. Workflow #453 S4's kind `new`
  is off it too: a root `new` is building is not started (a watchdog before the kind reads the
  file as unreadable and stands aside as well).
- `owner.lock` is only read, never locked. The ledger verification and check-config take their
  own locks as their contracts say.
- **`tools\actingwatch.exe` since Workflow #434 LAB-4.** It no longer speaks only through a command
  line: it runs the watchdog's tick itself, so it reads the selection (`install\active.json`) and
  the configuration it names itself and sends the Runtime `Health`
  ([runtime-watchdog.md](runtime-watchdog.md), "Launcher"). Today no A/B root is in use, and a
  single-location root replaces `runtime\` and `tools\` together, so actingwatch and the Runtime
  it watches come from one release: no interface revision now.

### Running images

`actingctl.exe` images (MCP servers, reads) are renamed aside while running and keep running;
`adb.exe` is never moved and `adb kill-server` is never used; any other running image under the
root that would change refuses (`running_image_blocks`); unchanged files are never
touched; MCP servers left on renamed images are listed so the person reconnects them (an MCP
entry switches its own child; #411 P49). The Lab's MCP server (Workflow #434 LAB-5G6, 0.12.57;
the coordinator's ruling 5 on board #434): an `actinglab-mcp.exe` image, in `tools\` or
`runtime\`, is renamed aside as an `actingctl.exe` image is, by the same rule (the file name), and
listed with them (`running.renamed` in the plan, `running_renamed` and the caveat
`mcp_clients_on_previous_build` after apply). It serves MCP by its image name, whatever its
command line: the process probe's one predicate marks `mcp_serve` for an `mcp-serve` command line
and for an image named `actinglab-mcp.exe`, also once renamed aside (`actinglab-mcp.exe.old-<ms>`),
so `install status` lists it among `mcp_servers_on_old_images` and the notice's
`mcp_clients_reconnect` counts it as an `actingctl.exe mcp-serve`; and the second MCP entry,
`actingcommand-mcp-entry-g2.exe`, is an entry as the first is, so a server it started asks no
client to reconnect (the coordinator's ruling of 2026-11-12). The rows keep their members (`pid`,
`path`, `mcp_serve`): `path` names the program. The Lab's rescue module (LAB-3 fix 1): the image
of the
process that plans, matched by its process id (`<root>\tools\actinglab.exe` running the module),
is renamed aside the same way and listed as `running.renamed_own`; another running
`actinglab.exe` still refuses. `actingctl install`'s planning is unchanged: its own image is
`actingctl.exe`.

### The rollback gate

A binary rollback (`rollback apply`, or `undo` of an update or rollback whose build has started)
restores exactly
the program files of `runtime\` and `tools\`, from a kept build or from the operation's
`.old-<ms>` files; never the state root, the ledger, `packages\`, `policy\` or the configuration
file (a configuration restore is `config restore`, a pack restore `pack restore`). The gate is
evaluated at `rollback plan`, again at apply under the writer lock, by `status
--check-rollback`, and by a composite's automatic rollback before it closes the new Runtime
(v0.12.25); all four parts must pass (`rollback_gate_part`):

1. `interface_edges`: the update edges with R\* the target and Rp the live build; the live
   build's `ledger` max ∈ the target's range. (Since 0.12.80 the row `requires`
   `next_names_required` is among them; failed, it refuses the plan first, "Subsystems the
   schedules require".)
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

The salt never leaves the configuration file and the candidate. The rescue module's `new`
(Workflow #453 S4) makes a fresh root's salt from 32 bytes of the operating system's generator and
writes it only into the first configuration's part. No device serial is read or
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

The Lab's rescue module (LAB-3) adds two members after `installer`, which `actingctl install`
never writes: `"initiator": "<word>"` (lab-two-entries.md §4.1) and `"one_way": true` (a one-way
ledger edge it applied; absent otherwise); its undo record carries `"initiator"` too, and
(Workflow #463 S3-18, since 0.12.69) the undo record of a `pack_place` operation
`"schedule_changes": {"enters": [...] | null, "leaves": [...] | null}` ("The packages the undo of
a placement makes enter or leave scheduling"; absent in a record of any other kind and in one an
earlier build wrote). Kinds
`config_replace` and `pack_place` are the module's, and (Workflow #453 S4) `new`, whose inputs
record the build, the folder it is copied from, the state root and the options, never the salt;
its `record_state` step's undo sets `state.json` aside (there is no previous state); and (Workflow
#453 S9, since 0.12.60) `vision_place` with its step `vision_file`. An older reader that refuses
unknown members does not read such a journal; `install-root` does not change (lab-two-entries.md,
open point 5).
LAB-3 fix 1: the evidence of the module's `pause` step carries `"own_pauses": {"reason",
"owner_epoch", "revisions": {"<alias>": <revision>}}`, the pauses it set; with the step's
`drained` it names the instances the module's release may hand back, matched by reason and alias
(fix 2: no start step records it any more, and the epoch and revisions are evidence only; "The
Lab's rescue module").

Workflow #454 (0.12.59; register §1.348; fix 1): the evidence of a `close` step that is not an
automatic rollback's carries `first_epoch` and, when they apply, `first_epoch_unread` and
`first_epoch_guard` ("The post-start checks (INST-2)"), written by the close of `actingctl
install` and of the rescue module alike; a `start` journal's `inputs` may carry `"first_epoch":
true`, carried from the close of the operation that stopped the Runtime. Both are free-form
values every version reads: `s0` and every other member keep the shape they have before 0.12.59,
so an older reader reads such a journal (its `undo`, `resume`, `status` and the S0 a standalone
start judges against included), and `install-root` does not change.

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
| `write_config` | Workflow #453 S4 (`new`): `actingd.config.json` is in place | check the part (written first when it is not there) with the live build's check-config, then rename it in | `actingd.config.json` → `.rolledback-<ms2>`; its part → `.abandoned-<ms2>` |
| `create_state_root` | Workflow #453 S4 (`new`): the state root's folder is there | create it | the folder → `<folder>.rolledback-<ms2>` when the operation made it |
| `create_vision_root` | Workflow #453 S4 fix round 1 (`new`): `<root>\vision` is there | create it | the folder → `vision.rolledback-<ms2>` |
| `vision_file` | Workflow #453 S9: `<dst>` holds the pin's SHA-256; a file of before is kept as `<dst>.old-<ms>` | a part that is not the file set aside `.abandoned-<ms>` and written again; finish whichever half is missing | `<dst>` holding the pin → `.rolledback-<ms2>`; `.old-<ms>` → `<dst>`; a part → `.abandoned-<ms2>` |

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
`actingctl mcp-serve`; the first generation's form is:

`<root>\mcp-entry\g1\actingcommand-mcp-entry.exe --root <root> --client <name> -- mcp-serve --tier …`

- **What it is.** A small program shipped in the Tools half (listed in its `BUILD-MANIFEST.json`).
  `g1` is the entry's own generation (`--generation` prints it); it changes only when the contract
  between the entry and its children changes (`entry_protocol`, 1 for `g1`). Each generation is a
  program of its own with its own file name, in its own `g<N>` directory: `g1\` only ever holds
  the first program, `actingcommand-mcp-entry.exe`, and `tools\actingcommand-mcp-entry.exe` is
  that first generation in every build; `g2\` only ever holds the second (below). Nothing
  overwrites a running entry. Placing an entry is a deploy step today (`g1\`: copy
  `tools\actingcommand-mcp-entry.exe`); placement by `install update` is a follow-up, and that
  step, when built, creates under `g2\` only and never writes `g1\`. The bullets below, up to
  and including "Registration", describe the first generation; the second differs from it as its
  own bullet, the last, says.
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
- **The second generation** (Workflow #434 LAB-5G5a, 0.12.57; the coordinator's ruling of
  2026-11-12 on board #434, which replaces ruling 6 of 6775192250). A second program,
  `actingcommand-mcp-entry-g2.exe`, the first generation's code with these differences only:
  `<root>\mcp-entry\g2\actingcommand-mcp-entry-g2.exe --root <root> [--client <name>] -- [<server words>…]`.
  - `--generation` prints 2, and it asks for `entry_protocol` 2.
  - Its child is `actinglab-mcp.exe`, looked for at each fresh start in `<root>\runtime\`, then in
    `<root>\tools\`, with every word after `--` unchanged and none required (`--tier` reaches the
    server, which refuses it). `--client` stays the entry's: the status file's `client` and the
    refresh entry, as for `g1`; no child is given it.
  - It watches the size and time of `runtime\BUILD-MANIFEST.json` and of the `actinglab-mcp.exe`
    it would start now (not `runtime\actingctl.exe`).
  - Its plan is `actinglab.exe --json mcp plan --entry-protocol 2 [--current-build <commit>]`,
    that program found the same way, with no `--root` (the rescue module acts on the root of the
    program that runs it) and no server words; the last line printed is the answer. Of the answer
    it reads `entry_protocol` (2; any other value makes the answer a refusal), `switch`, `build
    {commit, manifest_sha256}`, `child[0]` (the server program to start; the rest of `child` is
    not read, and the entry's own words follow it), `tunables` (the five members above; a
    refusal's too) and `outcome` (kept in the status file's `last_plan`); `reason` is not read.
    The rescue row `mcp plan` answers it (LAB-5G5b, the last bullet of this section).
  - Its own refusals are `mcp_entry_g2_outcome_unknown` (inspect with `ac_run_show` or
    `ac_status`), `mcp_entry_g2_child_unavailable` and `mcp_entry_g2_usage_invalid`: the meanings
    of `g1`'s three codes, owned by the second program.
  - Its status file is the same file with the same members, `<root>\mcp-entry\state\entry-<pid>.json`
    (`actingcommand.mcp-entry-status.v1`), with `entry_generation` and `entry_protocol` 2, so
    `mcp-entry list` lists both generations' entries side by side.
  - No release or Tools archive carries it before the packaging slice (LAB-5G6), and placing it in
    `g2\` is a manual step until an installer step exists. `actingctl mcp-serve` and
    `actingctl mcp-entry plan --entry-protocol 1` stay until the slice that removes actingctl,
    which comes after the owner's registrations have moved to `g2\`.
- **The rescue module's rows** (Workflow #434 LAB-5G5b, 0.12.57; register §1.287, "Slice
  LAB-5G5b"). Three rows of the operation table, read only, which `actinglab <row>` and `actinglab
  serve --stdio` serve on the root of the `actinglab.exe` that runs them (no `--root`, "The Lab's
  rescue module"). The first generation's `actingctl mcp-entry plan|list` and `mcp-config` answer
  as before.
  - **`mcp plan --entry-protocol 2 [--current-build <commit>]`** is the second generation's plan
    (operation `install_mcp_entry_plan`), and answers exactly what that entry reads: `{switch,
    build {commit, manifest_sha256}, child [server], tunables {poll_ms, settle_ms,
    verify_timeout_ms, drain_max_ms, refused_retry_max_ms}, entry_protocol 2, outcome}`, no
    `reason` and no other member. `switch` is true when `--current-build` names another build
    than the installed one (case-insensitive), false when it names the installed one or is not
    given. `child` has one element and takes no words: the server program `actinglab-mcp.exe` of
    the installed build, `<root>\runtime\actinglab-mcp.exe` when the Runtime member's manifest
    lists it, else `<root>\tools\actinglab-mcp.exe` when the Tools member's does (where the entry
    itself looks, in its order; a listed file is checked against its manifest with the rest of
    the build). Its rules and refusals are the first generation's plan's: the same order of
    checks, `layout_ab_unsupported`, `not_single_location`, `config_unreadable`,
    `install_section_invalid`, then with `tunables` `operation_unfinished`, `writer_lock_failed`,
    `live_build_invalid`, `live_build_modified`. A build whose manifests list no
    `actinglab-mcp.exe` (every build before the packaging slice) refuses
    `install_command_refused` with `install_refusal` `mcp_server_not_in_build` (`commit_sha` the
    build, `path` the last place looked), with `tunables`: never a path that does not exist,
    never `actingctl.exe`. Another `--entry-protocol` is refused `actingctl_usage_invalid`
    (`--entry-protocol`, `not_recognised`), as `actingctl mcp-entry plan` refuses one other than
    its own 1.
  - **The sample.** `crates/lab-rescue/tests/fixtures/mcp-plan-entry-protocol-2.json` is one
    answer of `mcp plan`, for a made-up root `C:\ActingCommand` whose build carries the server in
    its Tools member and a current build of another commit. The row's test produces it from such
    a root and compares; the second generation's test reads the same file with the entry's own
    reader and accepts it. The file is the contract between the two programs: a change to what
    the row answers or to what the entry reads changes it, and both tests.
  - **`mcp entries [--all]`** (operation `install_mcp_entry_list`) lists the entries' status files
    as `actingctl mcp-entry list` lists them, both generations side by side (the same file, the
    same members), each row with `generation` when its file states one (`entry_generation`).
  - **`mcp config --client claude|codex|antigravity|dsh [--list <module>[,<module>...]]`**
    (operation `install_mcp_config`) prints the second generation's registration snippet, in
    the shapes `actingctl mcp-config --entry` prints for the first:
    `<root>\mcp-entry\g2\actingcommand-mcp-entry-g2.exe --root <root> --client <client> --`, no
    `--tier` (the server has no tiers), and `--list <modules>` after `--` when `list` is given.
    `list` takes the
    server's `--list` words, the ids of the table's module rows of role `operations`: another
    word, an empty one or no word is refused `not_recognised`, a word twice `repeated`, each as
    `actingctl_usage_invalid` naming `--list`; an unknown client `--client` `not_recognised`. The
    answer is `{client, entry, snippet}` (the snippet's text as the client takes it); it writes no
    client file and reads none. While the program is not placed in `g2\`, the answer still gives
    the snippet and carries the warning `install_caveat_noted` (`install_caveat`
    `mcp_entry_not_placed`, the program as `path`), a `note` of its outcome and its `warnings`.

## Outcome codes

Registered by INST-0 in `contracts/outcome-codes/actingctl.json` (owner `actingctl`, `ladder_class`
`none`: request refusals and configuration of the person's own installer, decision-table row 7)
and `actingd.json` (`check_config_install_root_invalid`). Folded by ruling 54 (#420 S4, Alice:
「54就是按推荐做」): one domain code per kind of outcome; the value `operation`, a location
the fragment registers, names the command.

| Code | Category | Values |
|---|---|---|
| `install_command_done` | success | `operation`; `installation_changed` on a mutating command (`false`: nothing to do); `install_operation_id`, `commit_sha`, `tag`, `plan_id`, `count`, `game`, `alias`, `path`, `pid`, `owner_epoch`, `log` as the command has them |
| `install_caveat_noted` | warning | `operation`, `install_caveat`; `commit_sha`, `count`, `game`, `package_id`, `config_key`, `path`, `names` (Workflow #463 S3-18) as the caveat has them |
| `install_command_refused` | error | `operation`, `install_refusal`; `release_checks`, `rollback_gate_parts`, `ledger_sequence`, `install_guards`, `plan_id`, `install_operation_id`, `commit_sha`, `path`, `pid`, `count`, `files`, `game`, `package_id`, `alias`, `aliases`, `config_key`, `hold_reason` (LAB-3), `server` and `instance_index` (INST-5) as the reason has them |
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
- **Workflow #429 R-15b** (since 0.12.35) registered the codes and words of the A/B layout, which
  Workflow #429 AB-1 (0.12.78) removes with it. What stays: the keys `attempt_phase`,
  `hold_reason`, `s0_from`, `player_hold_range`, `confirm_argv`, `child_pid`, `child_state`,
  `child_exit_code`, `inherited_sha256`, `active_sha256` and `install_member` with the
  vocabularies `install_attempt_phase`, `entry_child_state` and `install_member` they name (the
  key table only grows); `release_check` `selection_schema_mismatch` ("Release verify and
  stage"); `install_guard` `player_holds` ([player-hold.md](player-hold.md), "Interactions"),
  which no guard list of this build evaluates; and the single location's words of two
  vocabularies:
  - vocabulary `install_operation_kind`: `release_stage`, `adopt`, `update`, `rollback`, `stop`,
    `start`, `release_hold`; the rescue module's composites add (Workflow #434 LAB-3, since
    0.12.48) `config_replace`, `pack_place`, its new row (Workflow #453 S4, since 0.12.60) `new`,
    and its vision place row (Workflow #453 S9, since 0.12.60) `vision_place`.
  - vocabulary `install_step_kind`: `extract`, `copy_build`, `verify_tree`, `place_build`,
    `pause`, `close`, `ledger_verify`, `stage_file`, `swap_file`, `record_state`, `start`,
    `post_check`, `release_hold`, `reverse_files`; the rescue module's place composite adds
    (Workflow #434 LAB-3, since 0.12.48) `pack_item`, its new row (Workflow #453 S4, since
    0.12.60) `write_config`, `create_state_root` and (fix round 1) `create_vision_root`, and its
    vision place row (Workflow #453 S9, since 0.12.60) `vision_file`.
- **`install_refusal`, INST-2**: `runtime_not_running`, `runtime_stop_failed`,
  `post_check_failed`, `deploy_rolled_back` ("Formal stop, start and the hold").
- **Workflow #434 LAB-3** (since 0.12.48; "The Lab's rescue module"): `install_caveat`
  `guard_failed` (`install_guards`); `install_refusal` `hold_not_owned` (fix 1; `hold_reason`,
  `count`, `aliases`), which only the rescue module answers; `install_command_refused` takes the
  existing key `hold_reason`.
- **Workflow #421 INST-5** (since 0.12.52; "Instances, the first slice"): `install_command_refused`
  takes the existing keys `server` and `instance_index` (`instance_invalid`); no code, location,
  key or word is new.
- **Workflow #434 LAB-5G5b** (since 0.12.57; "The MCP entry", "The rescue module's rows"): the
  location `install_mcp_config` (the rescue module's `mcp config`); `install_refusal`
  `mcp_server_not_in_build` (`commit_sha`, `path`), which only the rescue module's `mcp plan`
  answers; `install_caveat` `mcp_entry_not_placed` (`path`). `mcp plan` and `mcp entries` answer
  with the existing locations `install_mcp_entry_plan` and `install_mcp_entry_list`; no code or key
  is new.
- **Workflow #453 S5 and S6** (since 0.12.59; "The composition after a placement", "`place` while
  the Runtime is stopped"): with no Runtime answering, `install_pack_place` refuses with the
  existing `runtime_running` (`path`, `pid`), `owner_lock_held` (`path`) and
  `processes_unreadable`, no longer `runtime_not_running`; no code, location, key or word is new.
- **Workflow #453 S7** (since 0.12.60; "The Lab's rescue module", `vision manifest` and `vision
  check`): the locations `install_vision_manifest` and `install_vision_check`; `install_refusal`
  `vision_report_unanswered` (fix 1: either row, and since S9 `vision place`; actingd gave no usable report: its outcome or
  the child's failure as the cause, or `timeout_ms` for a child ended at its bound or at the
  `vision check` row's limit) and `vision_root_unresolved` (`vision check`, and since S9 `vision place`: actingd refused
  at a stage where it loads, assembles or resolves the vision configuration, `config_load`,
  `config_assemble` or `config_vision` (fix 2), its outcome as the cause), and `install_caveat`
  `vision_incomplete` (`count`), which only these rows answer; the existing `config_unreadable`
  and `pack_invalid` (`game`, `path`, `timeout_ms`) for the standard-pack root and a game root's
  check; no code or key is new here (actingd's `vision_manifest_reported` and
  `vision_check_completed` are actingd's own).
- **Workflow #453 S4** (since 0.12.60; "Fresh root"): the location `install_new` (registered by
  INST-0) is answered, by the rescue module's `new` only; `install_refusal` `root_not_empty`
  (`path`, and `install_operation_id` for a root holding an unfinished `new`) and
  `state_root_not_empty` (`path`), which only `new` answers; the existing `release_check_failed`,
  `build_invalid`, `interface_edge`, `config_refused_by_target`, `check_config_unanswered`,
  `owner_lock_held`, `runtime_running` and `processes_unreadable`, and the caveat
  `release_unconfirmed`; the operation kind `new` and the step kinds `write_config`,
  `create_state_root` and (fix round 1) `create_vision_root`; no code or key is new.
- **Workflow #453 S9** (since 0.12.60; "The Lab's rescue module", `vision place`): the location
  `install_vision_place`; `install_refusal` `vision_source_invalid` (`path`), which only this row
  answers; the operation kind `vision_place` and the step kind `vision_file`; it answers the
  existing `vision_root_unresolved`, `vision_report_unanswered` (`timeout_ms`),
  `owner_lock_held`, `runtime_running`, `processes_unreadable`, `operation_unfinished`,
  `writer_lock_failed` and `post_check_failed` (`count`); no code or key is new.
- **Workflow #453 S1** (since 0.12.60; "Instances, the first slice", "A first row"):
  `install_instance_add`'s `instance_invalid` gives the existing key `config_key` two more
  values, `policy` and `policy.resources.hosts`, beside those it gave before; no code, location,
  key or word is new.
- **Workflow #453 S3** (since 0.12.60; "Instances, the first slice", "While the Runtime is
  stopped"; "The Lab's rescue module", "The composites" and "A hand-back the Runtime answers by
  holding the instance again"): with no Runtime answering, `install_instance_add` refuses with the
  existing `runtime_running` (`path`, `pid`), `owner_lock_held` (`path`) and
  `processes_unreadable`, no longer `runtime_not_running` (which it still answers when a Runtime
  ran at its first read and none answers at the commit), and `install_config_replace` and
  `install_config_restore` refuse with the same three, as do `install_resume` and
  `install_undo` of a `config_replace` begun while no Runtime answered (fix 1); no code,
  location, key or word is new.
- **Workflow #453 S12** (since 0.12.60; "The Lab's rescue module", `state-root claim`): the
  location `install_state_root_claim`; `install_refusal` `state_root_claim_unanswered` (actingd
  gave no report: the child's failure as the cause, or `pid` and `config_key` for a child left
  running at its deadline) and `install_caveat` `fingerprints_discontinuous` (`path` the kept
  record), which only this row answers; the existing `owner_lock_held`, `runtime_running` and `processes_unreadable`
  for its stopped rule; no code or key is new.
- **Workflow #463 S3-2** (since 0.12.64; "The Lab's rescue module", "The authorization a
  placement names"): `install_refusal` `place_authorization_missing`, `place_enables_mismatch`
  (the answer's `expected` and `given`, not values) and `place_enables_unknown` (no cause is
  attached: why the lists cannot be stated is in the plan's `schedule_changes` beside the outcome,
  whose `unread` names the refusal, `standard_pack_root_invalid` at the file), each with `plan_id`
  and `game`, which only `install_pack_place` answers, each before the hold and any write; the
  existing `actingctl_usage_invalid` (`unusable`, `wrong_type`) for a malformed `--authorization`
  or `--enables`; no code, location or key is new.
- **Workflow #463 S3-18** (since 0.12.69; "The Lab's rescue module", "The packages the undo of a
  placement makes enter or leave scheduling"): `install_caveat` `undo_enters_scheduling` (`game`,
  `names` the packages that enter, sorted) and `undo_scheduling_unknown` (`game`, the failure of
  the game root as the undo leaves it as its cause; fix round 1: no cause when an undo run again
  after it stopped, its plan form or `op resume` answers it from the undo record, whose lists are
  `null`), each a warning, exit 0, a note of
  `install_command_done` of `install_undo` of a `pack_place` operation, done or planned
  (`--plan`), which only the rescue module answers; `install_caveat_noted` takes the existing key
  `names`; the existing `actingctl_usage_invalid` (`unusable`, `--plan`) for `--plan` of another
  kind of operation; no code, location or key is new.
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
