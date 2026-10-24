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
- **Read-only commands** write nothing: `release verify`, every `plan`, `status`, `guards`,
  `config show`, `config diff`, `leftovers`.
- **Mutating commands** each open one operation, journalled step by step (below). A root has
  at most one unfinished operation (`install\current.json`); every other mutating command then
  answers `install_operation_unfinished` with its `install_operation_id` and leaves the root
  alone.
- **Plan and apply.** `update`, `rollback`, `pack place`, `pack remove` and `config commit`
  apply only with `--plan <plan_id>` from a plan of the same inputs. `plan_id` is `plan-` and
  the first 16 hex digits of the SHA-256 of the canonical plan document (members sorted, no
  whitespace), which covers the inputs, the builds, the file and item actions, the interface
  edges, the defaults diff, the configuration SHA-256 values and the check-config summaries;
  process ids, times and the rollback gate's ledger figures are not in it. Apply recomputes the
  plan under the writer lock and refuses `install_plan_changed` on any difference.
- **Guards** (INST-2). Each command lists its guards as `guards: [{id, result: pass | fail |
  not_evaluated, values}]`. A `fail` or `not_evaluated` refuses with `install_guard_failed`
  unless the person names it with `--ack-guard <id>`; acknowledgments are journalled.
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
   usage error). Without facts the result is the warning `install_release_verified_local`: tag
   and digests not checked.
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

Results: `install_release_verified` (`commit_sha`, `tag`) with the result members `commit`,
`tree`, `tag`, `prerelease`, `interfaces` and `files` (the count), or `install_release_refused`
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
Staging a build that is already kept and verifies answers
`install_build_staged` with `unchanged: true` and opens no operation. A root that
`release stage` writes need not be adopted yet: it writes only `install\`.

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
`install_live_build_modified` (`count`, `files`). A second adopt of the same live build is
`install_root_adopted` with `unchanged: true`. Adopt is a journalled operation (`kind` `adopt`:
`copy_build`, `place_build`, `record_state`).

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
[--known-pause ALIAS=REASON]… --start --ack-guard <id>` to apply, and INST-3 adds
`--with-config`; until then apply needs the Runtime stopped and leaves the configuration alone.

**Plan** (read-only; `--preview` allows a running Runtime and reports it instead of refusing):

- **Builds.** The live build: the `runtime\` and `tools\` manifests name one commit and every
  listed file verifies (`install_live_build_modified` otherwise). The target:
  `install\builds\<commit>` verified again (`install_update_refused`, `build_not_kept` or
  `build_invalid`). The root must be adopted (`not_adopted`) and single-location
  (`not_single_location`; with `install\active.json` `install_layout_ab_unsupported`).
- **Interface edges** (below), one row each in `edges`. A failed edge refuses
  `install_update_refused` (`interface_edge`). An unknown pack declaration is the warning
  `install_pack_declaration_unknown` (`game`).
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
  that differ. The target refusing refuses the plan (`config_refused_by_target`, its outcome as
  the link); no answer refuses `check_config_unanswered`.
- **Ledger verify.** When the target's `ledger` range differs from the live one, apply first
  runs the target's `actingd ledger-maintenance verify --config <root>\actingd.config.json`
  (cold) and refuses `ledger_verify_failed` unless it exits 0 (the acsetup gate of
  [component-interfaces.md](component-interfaces.md), "Checks per operation").
- **Running state.** An `actingcommand-actingd.exe` under the root (`runtime_running`) or a held
  owner lock (`owner_lock_held`) refuses unless `--preview`. Images under the root that would be
  replaced while running: `actingctl.exe` is renamed aside and keeps running (listed as
  `running_renamed`); `adb.exe` is never moved and any other image waits
  (`install_running_image_blocks`, `path`, `pid`). A `.part` of another operation under
  `runtime\` or `tools\` refuses `install_foreign_part` (`path`).
- **Notice block** for the person's notice before the change: what changes, whether MCP servers
  must reconnect, and whether the rollback window will be bounded (a one-way ledger edge).

**Apply.** Steps in order, each journalled:

1. The preconditions above, under the writer lock (2), and the plan recomputed: a different
   `plan_id` refuses `install_plan_changed`.
2. `install\writer.lock` exclusive, created when missing; contention waits
   `install.lock_wait_ms` (`writer_lock_failed` after it).
3. (INST-2: `--stop`.)
4. **S0**: the ledger head, read with the live build's `actingledger --state-root <state> open`
   (`latest_sequence`), recorded in the journal's `s0.head` (`ledger_head_unreadable` refuses).
5. `ledger_verify`, when planned.
6. `stage_file` for every `new` or `replace` file: the kept build's file copied to
   `<dst>.new-<ms>.part` and its SHA-256 checked.
7. `swap_file`, `tools\` first, then `runtime\`: a replaced `<dst>` renamed `<dst>.old-<ms>`,
   then the part renamed `<dst>`. Unchanged files are never touched.
8. `verify_tree`: every manifest file in place.
9. `record_state`: `state.json`'s live build is the target, installed by this operation.
10. (INST-3: `--with-config`; INST-2: `--start`.)
11. The lock released and `current.json` renamed `current.json.finished-<ms>`.

Result: `install_update_applied` (`install_operation_id`, `commit_sha`) with the members
`operation_id`, `from_commit`, `to_commit`, `replaced`, `added`, `verified`, `running_renamed:
[{pid, mcp_serve}]` and `next` (the undo form). When `running_renamed` is not empty the warning
`install_mcp_clients_on_previous_build` (`count`) is a `note` link. A live build equal to the
target is `install_update_unchanged` and changes nothing. A step that fails after the first
change on disk leaves the journal and `current.json` and ends `install_operation_interrupted`
(fatal, `install_operation_id`) with the step's failure as its link: the root cannot run normally
until `resume` or `undo`.

**Rollback** `--to <commit>` is an update to a kept build, with the rollback gate (below) in its
plan and evaluated again under the writer lock immediately before the first change. A failed
gate is `install_rollback_refused` (`rollback_gate_parts`), or
`install_rollback_ledger_unreadable` (`ledger_sequence`) when the target's reader refused a page:
forward fix only. Success is `install_rollback_planned` / `install_rollback_applied`.

**Undo and resume.**

- `undo` reverses an unfinished operation, or the last finished one when no operation followed
  it; for a finished update or rollback the rollback gate must pass (it passes trivially when no
  Runtime owner started since the operation, read from the owner journal). Every step is reversed
  as the step table says; every displaced file is kept. Result `install_operation_undone`.
- `resume` continues an unfinished operation: each step's postcondition is checked on disk, a
  step already true is done, a missing half is finished. Result `install_operation_resumed` with
  the operation's own result members.
- An operation id that names no journal, an operation `resume` cannot continue (finished) or one
  `undo` may not reverse (not the last) is `actingctl_usage_invalid` (`argument` `--operation`,
  `argument_problem` `unusable`).

**Status** (`install_status_reported`) answers the layout, the live and kept builds, the pack
register, the committed configuration SHA-256 and whether a candidate exists, the unfinished
operation, the last finished operations, the leftovers count, and the rollback window's cheap
parts (the interface edges toward each kept build, whether an owner started since the live
build's operation). `--operation <id>` answers that journal. `--check-rollback` evaluates the
whole gate toward the previous build.

**Leftovers** (`install_leftovers_reported`, `count`) lists every kept file and folder with its
operation id and size. It deletes nothing.

### `actingd check-config --install-root <root>` (INST-1)

check-config judges the install root from its own executable; from
`install\builds\<c>\runtime\` that is the kept build's folder. `--install-root <root>` judges it
as `<root>`, exactly as a daemon running from `<root>\runtime` would
([actingd-check-config.md](actingd-check-config.md), "Invocation").

## Formal stop, start and the hold (INST-2)

```text
actingctl install stop  --root <root> --hold <reason> [--known-pause ALIAS=REASON]... [--ack-guard <id>]...
actingctl install start --root <root> [--ack-guard <id>]...
actingctl install release-hold --root <root> --reason <reason>
actingctl install guards --root <root> --for stop|start|hot-place [--game <game>]
```

- **Stop**: S0 (`status`: the ledger head, the pauses, the run-time instructions) journalled; every
  unpaused instance paused with reason `--hold` and drained `install.stop.drain_timeout_ms`;
  `status` must show exactly the expected pauses; the close is the upgrade state `enter` inside an
  update when I and the Runtime both speak `install-control` 2, otherwise (and outside an update
  always) `request-shutdown`; gone within `install.stop.shutdown_wait_ms` with the terminal
  outcome `runtime_stopped`. Result `install_runtime_stopped`. A failed step keeps the hold
  pauses and names `release-hold`.
- **Start**: no live owner, no actingd under the root, the configuration passes the check-config
  of `runtime\`; the `memory` guard before every attempt; `writer.lock` and
  `<root>\watchdog\run.lock` held; the watchdog's manual start with the log
  `<root>\watchdog\actingd-<ms>.log`; readiness every 500 ms until
  `install.start.ready_timeout_ms`; an error terminal outcome starts the next attempt up to
  `install.start.attempts`, a fatal one does not; the post-check (`post_check: [{id, result,
  values}]`, a mismatch the warning `install_post_check_failed`). Result
  `install_runtime_started` (`pid`, `owner_epoch`, `log`), or `install_runtime_start_failed` /
  `install_runtime_start_timeout` (the process left running).
- **release-hold** resumes only the instances paused with exactly `--reason`
  (`install_hold_released`). `install.hold_after_start` keeps a composite `update apply --stop
  --start` held until the person releases it.
- Tools pauses only with its own reasons (`--hold`, `install-<operation_id>`) and resumes only
  those; another party's pause is kept and allowed only when named by `--known-pause`; a global
  pause refuses.

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
  file and records its SHA-256 in `base.json` (`install_config_base_changed` once the live file
  changed). `set` takes only keys of the running installer's key table; the Lab options are
  written only by `lab-options` (`install_config_lab_options_locked`); `secret_fingerprint_salt`
  is never accepted, printed, diffed or journalled. `commit` plans with the check-config of the build that will run
  (`--install-root <root>`; `install_config_refused`, `install_catalog_plan_unexpected`), then
  applies under the writer lock: candidate → `actingd.config.json.part`, live →
  `.prev-<ms>`, part → live, read back (`install_config_committed`; with the Runtime running
  only with `--while-running`, which reports `install_restart_required`). `restore` commits a
  kept `.prev-<ms>` the same way. Installer commands never set run-time instructions.
- **Instances** (INST-5, on INST-R). `add` and `remove` write the candidate only; a commit
  follows. A minted `instance_id` is never reused; there is no default backend; no device serial
  is read or written (`install_instance_staged`, `install_instance_refused`).
- **Packs** (INST-4). Plan with the contract crate's own readers (`BundleIndex::validate`,
  `read_standard_pack_root`), the package interface edge, the delta per item and the
  Runtime-semantic rules (`install_pack_binding_broken`, `install_pack_maintenance_drift`,
  `install_pack_restart_needed`); place cold or hot in the reader's order, each item `.part` →
  `.old-<ms>` → in place; the register records game, source repository, commit, tag and the
  declared `package` range, never a digest (`install_pack_placed`, `install_pack_not_idle`,
  `install_pack_full_check_unavailable`). `remove` renames the game root
  `packages\<game>.old-<ms>`; `restore` puts one pack operation's kept items back.
- **Fresh root** (INST-6). `new` refuses a non-empty root or state root, copies and verifies the
  build into `runtime\`, `tools\` and `install\builds\<c>\`, writes the first configuration (the
  salt 32 bytes from the OS CSPRNG, never shown), runs its check-config and writes `state.json`;
  nothing is started (`install_root_created`). The vision and OCR models come from
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
- **Row 7a″.** While `install\current.json` names an unfinished operation, a tick that would
  consider a start answers `watchdog_installer_unfinished` (warning, `install_operation_id`) and
  starts nothing, and `actingctl start` and `restart` refuse the same way
  ([runtime-watchdog.md](runtime-watchdog.md), "Decision"). The OS releases the writer lock when
  an installer dies, so without the row the watchdog would start a half-swapped root.
- `owner.lock` is only read, never locked. The ledger verification and check-config take their
  own locks as their contracts say.

### Running images

`actingctl.exe` images (MCP servers, reads) are renamed aside while running and keep running;
`adb.exe` is never moved and `adb kill-server` is never used; any other running image under the
root that would change refuses (`install_running_image_blocks`); unchanged files are never
touched; MCP servers left on renamed images are listed so the person reconnects them.

### The rollback gate

A binary rollback (`rollback apply`, or `undo` of a finished update or rollback) restores exactly
the program files of `runtime\` and `tools\`, from a kept build or from the operation's
`.old-<ms>` files; never the state root, the ledger, `packages\`, `policy\` or the configuration
file (a configuration restore is `config restore`, a pack restore `pack restore`). The gate is
evaluated at `rollback plan`, again at apply under the writer lock, and by `status
--check-rollback`; all four parts must pass (`rollback_gate_part`):

1. `interface_edges`: the update edges with R\* the target and Rp the live build; the live
   build's `ledger` max ∈ the target's range.
2. `ledger_readability`: S0 is the ledger head the operation that installed the live build
   recorded (its journal's `s0.head`). The **target build's own** `actingledger --state-root
   <state> events --after <S0> --limit 1024` reads every page up to the head with exit 0; the
   first page it refuses closes the window (`install_rollback_ledger_unreadable`, its first
   sequence). A live build no journalled operation installed has no S0, and this part fails.
3. `open_time`: the target's open deadline is the configuration's
   `ledger_observer.open_timeout_ms` when the file names it, else the target's
   `actingd.defaults.json` value; a target whose defaults lack the key is a build before v0.12.11
   and fails. The newest `ledger_opened` daemon line of the live build's starts (its
   `sql_read_ms + verify_ms + material_ms + restore_ms`, scaled by the head over its
   `event_count`) must be at most the deadline minus `install.rollback.open_margin_ms`.
4. `config`: the live configuration passes the target's check-config (`--install-root
   <root>`); a key of a newer `actingd-config` revision fails here, so the person restores the
   configuration first.

A failed gate leaves a forward fix only.

### Guards (INST-2, INST-4)

| id | Check | Key | Commands |
|---|---|---|---|
| `memory` | available physical memory ≥ the key (the watchdog's hidden CIM query) | `install.guard.min_available_memory_mb` | start (each attempt), update with `--start` |
| `schedule_near` | no affected instance due within the key (the Runtime's schedule view; `not_evaluated` without a Runtime) | `install.guard.before_next_start_ms` | stop, hot place, update with `--stop` |
| `blocked_window` | now (host local time) in no configured window | `install.guard.blocked_windows` | stop, start, hot place |
| `foreign_pauses` | no global pause; no unnamed pause of another party on an affected instance | — | stop, hot place |
| `ladder_holding` | no affected instance in a ladder busy hold | — | hot place |
| `adb` | each configured instance with a known port answers `shell echo ok` through an adb server already running from `<root>\tools\platform-tools\`; no server is `not_evaluated`; Tools never starts, connects or kills one | `install.guard.adb_timeout_ms` | stop, start, hot place |

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
 "operation_id": "op-<ms>", "kind": "release_stage" | "adopt" | "update" | "rollback",
 "root": "<root>", "installer": {"commit": "<40 hex>" | null, "path": "<path>"},
 "inputs": {…}, "plan_id": "plan-<16 hex>" | null, "plan": {…} | null,
 "guards": [], "acknowledged": ["accept_ledger_one_way", …],
 "s0": {"head": 123, "epoch": null, "pauses": null, "instructions": null} | null,
 "previous": {"live": "<40 hex>" | null, "state": {…} | null},
 "steps": [{"n": 1, "kind": "<step>", "target": "<relative path>", "intent": {…},
            "state": "planned" | "started" | "done" | "undone", "evidence": {…} | null}],
 "undo": {"started_at_unix_ms": 0, "finished_at_unix_ms": 0 | null} | null,
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
| `pause`, `close`, `start`, `post_check`, `release_hold` | INST-2 | | |
| `config_swap` | INST-3 | | |
| `pack_item` | INST-4, as `swap_file` per item | | |

`<ms2>` is the undo's own Unix milliseconds. Crash points: before the first swap the live tree is
untouched (resume restages, undo abandons the parts); between swaps every file is old or new,
never torn (a rename is atomic); after the swaps the new tree is complete and the Runtime stopped
(resume finishes, undo rolls back by record). While `current.json` exists every other mutating
command refuses and the watchdog stands aside (row 7a″). A `.part` of another operation refuses
(`install_foreign_part`); Tools never removes it.

## Configuration keys

The section `install` of `actingd.config.json` ([runtime-config.md](runtime-config.md), "The
`install` section"; `actingd-config` revision 11). actingd parses and validates it and otherwise
ignores it; `actingctl` reads it from the root's configuration file through
`actingcommand-runtime-config`. Fixed by this contract, not tunable: the readiness probe period
(500 ms), the 2 s clock slack, the layouts, schema names and file names, the ledger page of the
gate (1024 events).

## Outcome codes

Registered by INST-0 in `contracts/outcome-codes/actingctl.json` (owner `actingctl`, `ladder_class`
`none`: request refusals and configuration of the person's own installer, decision-table row 7)
and `actingd.json` (`check_config_install_root_invalid`).

| Category | Codes |
|---|---|
| success | `install_release_verified`, `install_build_staged`, `install_root_adopted`, `install_update_planned`, `install_update_applied`, `install_update_unchanged`, `install_rollback_planned`, `install_rollback_applied`, `install_operation_resumed`, `install_operation_undone`, `install_runtime_stopped`, `install_runtime_started`, `install_hold_released`, `install_guards_reported`, `install_config_staged`, `install_config_planned`, `install_config_committed`, `install_config_restored`, `install_pack_planned`, `install_pack_placed`, `install_pack_removed`, `install_pack_restored`, `install_instance_staged`, `install_root_created`, `install_status_reported`, `install_leftovers_reported` |
| info | `install_restart_required`, `install_pack_full_check_unavailable` |
| warning | `install_release_verified_local`, `install_mcp_clients_on_previous_build`, `install_post_check_failed`, `install_pack_declaration_unknown`, `install_pack_restart_needed`, `install_config_overridden_by_instruction`, `watchdog_installer_unfinished` |
| error (nothing changed) | `install_release_refused`, `install_update_refused`, `install_rollback_refused`, `install_rollback_ledger_unreadable`, `install_plan_changed`, `install_guard_failed`, `install_operation_unfinished`, `install_foreign_part`, `install_running_image_blocks`, `install_live_build_modified`, `install_layout_ab_unsupported`, `install_config_refused`, `install_config_base_changed`, `install_config_lab_options_locked`, `install_catalog_plan_unexpected`, `install_pack_refused`, `install_pack_binding_broken`, `install_pack_maintenance_drift`, `install_pack_not_idle` (the instances stay paused), `install_instance_refused`, `install_runtime_start_failed`, `install_runtime_start_timeout`, `check_config_install_root_invalid` (actingd) |
| fatal | `install_operation_interrupted`: a step failed after a change on disk; the journal names `resume` and `undo`, and until one runs the root cannot run normally |

Keys: `install_operation_id` (id), `plan_id` (id), `install_refusal`
(vocabulary `install_refusal`), `release_checks` (list of `release_check`), `rollback_gate_parts`
(list of `rollback_gate_part`), `ledger_sequence` (integer), `install_guards` (list of
`install_guard`). A refusal's causes chain the Runtime's or the contract's own code: check-config's
outcome, a child process's failure (`foreign_other_error`), an OS error (`foreign_os_error`).
