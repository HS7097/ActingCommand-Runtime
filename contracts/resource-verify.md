# `actinglab resource verify`

Workflow #411 RV-1 (register [v012-register.md](v012-register.md) §1.126). One command checks one
commit of a resource repository the way the repository's CI does, in one process, with this
build's own checks, and writes a report bound to the commit, its tree, its base and the build.
The repository wrapper keeps only what is policy of the repository (below, "What stays in the
wrapper").

## The command

```text
actinglab [--json] resource verify --repo <checkout> --source-repository <owner>/<name>
    --work <new directory> --report <new file> [--commit <40-hex>] [--base <commit-ish>]
```

- **What it is.** It checks one commit of a resource repository the way the repository's CI
  does, in this process, with this build's own checks. The repository is read as the **source of
  a standard-package bundle**:
  - the root inputs of `package bundle`: `applications.json`, `maintenance.json` and the root
    files `pages.json`, `interrupts.json`, `navigation.json`, (ROUT-L RL-1a) `schedule.json`,
    (Workflow #392 DA-4) `display-layout.json` and (Workflow #407 LOAD-1) `loading.json`, given
    as `--loading` with the templates of its `loading/` directory, when present; the bundle's
    root then holds the `loading/` directory too, and the installed root holds it beside
    `loading.json`;
  - (Workflow #444 NOTICES-3) the notices directory `notices/`, the package author's notice
    texts, given as `--notices` when present; the bundle's root, the zip and the installed root
    then hold it too, and the zip's `interfaces.json` declares `package` [10, 10];
  - `packs/<package_id>/`, the bundle index v4 `path` form;
  - `declaration-exclusions.json`;
  - `regression/`, the regression cases (their checks arrive with Workflow #411 RV-2a).

  It knows no other layout and no game: the game is `applications.json`'s `game`.
- **`--repo`** is the top of a Git work tree (a linked worktree is fine) whose HEAD is the commit
  to verify. **`--commit`**, when given, must equal HEAD.
- **Clean.**
  - No tracked change anywhere.
  - No untracked file anywhere.
  - No ignored file under `packs/` or `regression/`, and no ignored root input.
  - Every file under `packs/` and `regression/` and every root input (the five above,
    `declaration-exclusions.json` and `manifest.yaml`) is the committed blob, byte for byte (no
    filters).

  Otherwise the command fails the `checkout` step (`resource_verify_checkout_dirty`), and no
  other step runs: nothing read from such a tree is bound to the commit.
- **`--base`.** Without it, the declarations are every tracked `packs/**/*.json` plus the root
  declaration files the commit tracks (`applications.json`, `maintenance.json`,
  `declaration-exclusions.json`, `pages.json`, `interrupts.json`, `navigation.json`): the CI's
  manual-run selection, with the root files added. With it:
  - the base must name a commit that is an ancestor of HEAD (`resource_verify_base_invalid`);
  - the declarations add `git diff --no-renames --name-status -z <base> HEAD -- '*.json' '*.yaml'
    '*.yml'`, the CI's pull-request selection with its deletions (`D` is a removal; `A`, `M` and
    `T` are not; any other status is `resource_verify_git_failed` with `git_command` `diff`).
- **`--source-repository`** is written into `bundle.json` `source` as a release writes it, with
  HEAD as the commit.
- **`--work` and `--report`.**
  - Both must not exist, nor `<report>.part`, and their parents must exist
    (`resource_verify_output_exists`).
  - The command writes only below `--work` and the report. The report is written as
    `<report>.part`, then renamed, on success and on failure alike. Nothing is deleted; a failed
    `package bundle` leaves its `bundle.part` as `package bundle` does.
- **Git.** The command runs the `git` found on `PATH`, in `--repo`, with `GIT_OPTIONAL_LOCKS=0` and
  `-c core.quotepath=off`, read-only plumbing only: `rev-parse` (`--show-toplevel`, `--verify
  --quiet` of `HEAD^{commit}`, `HEAD^{tree}` and `<base>^{commit}`), `merge-base --is-ancestor`,
  `status --porcelain=v1 -z --untracked-files=all --ignored=matching`, `ls-files -s -z`, `ls-files
  -z -- 'packs/*.json'`, `ls-files -z -- 'packs/*.json' '*procedure-manifest*.json'`,
  `hash-object --no-filters --stdin-paths` (one call for every read path) and `diff`. It never
  checks out, fetches or writes the index, a ref or the work tree; LFS is not involved.
  `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_COMMON_DIR` are not passed on, so `--repo`
  alone names the repository. A git that cannot start, or ends with a code that is no answer of
  its command, is `resource_verify_git_failed`.
- **Every step runs.**
  - Only a failed `checkout` precondition leaves the content steps `not_run`.
  - There is no switch that skips, selects or stops early.
  - Steps do not stop at their first finding when the underlying check can go on:
    `declarations`, `admission`, `format`, `pins` and `bundle`'s index and read-back checks list
    every failure.
- **Result.** Since Workflow #434 LAB-2b the command prints the CLI module's line
  ([lab-two-entries.md](lab-two-entries.md) §9.3), and every error exits 1; with `--via` (MCP's
  actinglab child, which does not run this command) the envelope and the exit codes below in
  parentheses stay until LAB-5.
  - **Exit 0:** the line's member `report` is the whole report, unchanged, beside the line's own
    `outcome` `lab_operation_completed`: the report has its own top-level `outcome`, so it is never
    flattened (the coordinator's ruling of 2026-11-10 on LAB-2b; envelope: `ok: true`, the data is
    the report).
  - **Exit 1 (2), a failed step:** `resource_verify_failed`. `details` is the report; its outcome's
    causes are each failed step's first failure, with `step`.
  - **Exit 1 (2), a precondition:** `resource_verify_checkout_invalid`,
    `resource_verify_checkout_dirty`, `resource_verify_base_invalid` or
    `resource_verify_git_failed`, with the report in `details`.
  - **Exit 1 (2), a usage error before anything is read:** `validation_failed` or
    `resource_verify_output_exists`, and no report.
  - **A report that cannot be written:** `resource_verify_report_write_failed` (exit 1 (2)).
    stdout still carries the whole result: `details` is the report.
- **Refusals.**
  - Global `--dry-run`: `dry_run_unsupported`, after flag parsing only, with no alternative (the
    command writes its work directory and report).
  - Device, Runtime and instance options (`--instance`, `--instances`, `--runtime-endpoint`,
    `--capture-backend`, `--touch-backend`, `--game`, `--server`, `--profile`, `--resource-root`,
    `--run-root`): `offline_device_scope_forbidden`, as for `replay`.
  - Any other flag, a flag given twice, a flag without its value, or a positional argument:
    `validation_failed`. `--commit` must be 40 lowercase hexadecimal characters,
    `--source-repository` `<owner>/<name>` of GitHub name segments, and `--base` at most 256
    bytes that do not start with `-`.
- **Load and isolation.** It opens no Runtime connection, touches no device, writes no ledger and
  executes no repository code. It runs as one process, single-threaded but for the writer of the
  one `hash-object` call's stdin (one `labslot` slot).

## Steps

| Step | Runs | Reuses (in process) | Versus CI |
|---|---|---|---|
| `tools` | Reports `runtime_head` (compiled in), `cli_version`, `runtime_version`, the executable and its SHA-256. When `BUILD-MANIFEST.json` lies beside the executable, its `commit_sha` must be this head and its `actinglab.exe` entry's `sha256` this executable's bytes, otherwise `resource_verify_tools_mismatch` (`commit`, `executable_sha256`, or `manifest_invalid` when it does not read as a build manifest). Without the manifest `build_manifest` is `null`, which is no failure. | `ACTINGCOMMAND_RUNTIME_HEAD`, `sha2` | new: lets the wrapper bind the report to the pin without trusting the wrapper |
| `checkout` (precondition) | the top level; `HEAD^{commit}`, `HEAD^{tree}`; `--commit`; `--base` resolved and an ancestor; a clean status; the no-filter blob identity of the read paths (`bundle.ps1:236-257`) with **one** `git ls-files -s -z` and one `git hash-object --no-filters --stdin-paths`. A missing read file is a `blob_mismatch`. One `resource_verify_checkout_dirty` per kind found. | git plumbing | CI checks out fresh; this proves the same of a local tree |
| `declarations` | the selection (above), built in process from `git ls-files` / `git diff --name-status -z` as (path, deleted) pairs, so no list file is written and the checkout stays clean. It goes through the reader of `resource validate`, once over the whole selection, exactly as CI does. If that fails, every remaining path (after the one it stopped at) is validated on its own, so each further invalid declaration is named too, with `found_after_first: true`; the step's first failure is the one CI reports. Warnings (`resource_pool_producer_missing`) are carried. | `resource_declarations::validate_declaration_selection`, the reader of `resource validate` taking a selection | the same verdict as CI; lists more |
| `pins` | the rule of the resource repositories' `resource-declarations.yml:67-101`, typed: every file `git ls-files -z -- 'packs/*.json' '*procedure-manifest*.json'` lists carries no member named `package_digest`, and a file under `packs/` no member named exactly `sha256`, at any depth; every member is visited, a duplicated member each time; a BOM is accepted. Each hit is one `resource_verify_pin_committed` (`path`, `json_pointer` per RFC 6901, `member`); a file that is not UTF-8 JSON is `resource_declaration_invalid` naming it. The Runtime names a package by its path and ignores these keys until PKG-S1b refuses them again ([selection-graph.md](selection-graph.md), the select step's `policy`). | new; a JSON walk | identical rule |
| `admission` | the layout of `packs/` (`bundle.ps1:221-234`): pack directories and `packs/README.md` only, `applications.json`, `maintenance.json` and `packs/` present; for **every** pack: admitted as `package preflight` admits it, its `package_id` the directory name, its game `applications.json`'s and its server declared there. Every refused pack is listed: `package_invalid` (caused by the admission's own code) or `resource_verify_layout_invalid` for the layout and identity rules. | `package_cli::preflight_admission` (`contained_resources::load` + `PreparedContainedTask::from_bundle`) | superset of CI's touched-pack load; equal to `bundle.ps1:259-289`, but lists every refused pack |
| `format` | `package fmt --check` of every pack directory: one `package_fmt_check_failed` per pack with unformatted files, with `path` and `unformatted` | `package_tool::canonical_changes` | identical |
| `bundle` | `bundle.ps1:189-465` with the root files and the installed root, under `--work`: `packs-root/` (the pack directories staged), `bundle/` (`package bundle` with `applications.json`, `maintenance.json`, the source and the root files present, and (Workflow #444 NOTICES-3) the repository root's `notices/` as `--notices` when it holds one, found as the root files are), the index and copy checks (`resource_verify_bundle_mismatch`), `out/<game>-resources-<sha7>.zip` (entries in ordinal order, `/`-separated, the `notices/` files among them, with `interfaces.json` `{schema_version actingcommand.component-interfaces.v1, speaks {package [3, 3]}, validated_with {repository HS7097/ActingCommand-Runtime, commit <runtime_head>}}` beside `bundle.json`, LF-terminated; `package` [10, 10], the contract crate's `NOTICES_PACKAGE_REVISION`, when the bundle carries `notices/`) and `out/SHA256SUMS` (`<hex>  <name>\n`), `read-back/<game>/` (the same entries and bytes, and `interfaces.json` as written: `resource_verify_readback_mismatch`), `installed/<game>/` (the root files beside `bundle.json`, the `notices/` files in `notices/`, each `packs/<id>/` as `<id>/`) checked by `standard-pack check` (its `root_files` must be `bundle.json` and the root files given, and its `notices` the count and bytes of the zip's `notices/` files, absent without them; its warnings `package_bundle_entry_page_unlisted`, `package_bundle_interrupt_page_shadowed` (since 0.12.28) and `package_bundle_remedy_invalid` are carried), and every read-back pack admitted again, stating the indexed `package_id`, server and entry task. Without `notices/` every byte the step writes is what it wrote before NOTICES-3. | `actingcommand_resource_tooling::package_bundle`, `check_standard_pack_root`, `package_cli::preflight_admission`, `zip` | for a release script that checked the zip layout, it replaces that check by the installed root the Runtime's registry reads |

The index and copy checks (`resource_verify_bundle_mismatch`, its `bundle_problem`):
`index_schema` (bundle.json is no bundle v4 index); `maintenance_differs` (the maintenance read
back, from the answer and from `bundle.json`, differs from `maintenance.json`, order of members
aside); `root_file_differs` (the bundle root holds other entries than `applications.json`,
`bundle.json`, `packs`, the root files given and (Workflow #444 NOTICES-3) `notices` when the
repository holds it, or a root file differs from its source);
`applications_differs` (the bundled `applications.json` bytes, the index's game, or a game that is
no directory name); `source_differs`; `pack_list_differs` (the indexed packs are not the pack
directories, or a pack's path, server or entry task differs from its source); `pack_copy_differs`
(a copy is not its source, file for file and byte for byte); `pack_counts_differ` (`file_count`
or `byte_count`); `default_package_unmatched` (a server's `default_package_id` names not exactly
one bundled pack of that server); `root_files_unread` (`standard-pack check` of the installed
root read other root files, another number of packs or another game, or (Workflow #444
NOTICES-3) counted other notices files or bytes than the zip carries). A pack is copied only as
`control.json` and `resources/`, so any other file in a pack directory is a `pack_copy_differs`.

Each step's failures are outcome envelopes, either existing codes or the codes below. A failure
carries `step` where its code declares the key.

## Report

`actinglab.resource-verify.v1`, a Lab-side file and the CLI answer's data (on success) or
`details` (on failure); never a ledger record.

```json
{
  "schema_version": "actinglab.resource-verify.v1",
  "result": "passed | failed",
  "outcome": "<actingcommand.outcome.v1 envelope, full detail: resource_verify_passed / resource_verify_failed / a precondition code>",
  "started_at_unix_ms": 0,
  "finished_at_unix_ms": 0,
  "repository": {
    "root": "<absolute>",
    "commit": "<40 hex>",
    "tree": "<40 hex>",
    "base": null,
    "source_repository": "<owner>/<name>",
    "game": "<applications.json game, as data>"
  },
  "tools": {
    "cli_version": "…",
    "runtime_version": "…",
    "runtime_head": "<40 hex, compiled in>",
    "executable": "<absolute>",
    "executable_sha256": "<64 hex>",
    "build_manifest": null
  },
  "work": "<absolute>",
  "steps": [
    {
      "step": "tools | checkout | declarations | pins | admission | format | bundle",
      "result": "passed | failed | not_run",
      "duration_ms": 0,
      "counts": {},
      "failures": ["<outcome envelopes>"],
      "warnings": ["<outcome envelopes>"]
    }
  ],
  "failed_steps": ["…"]
}
```

- **`repository.commit`, `tree`** are `null` when the precondition failed before it knew them;
  **`game`** is `null` when `applications.json` does not read.
- **`base`** is `{"rev": "<as given>", "commit": "<40 hex>"}` when `--base` is given (`commit`
  `null` when it names no commit).
- **`build_manifest`**, when the file lies beside the executable: `{"path", "sha256",
  "commit_sha", "actinglab_sha256"}`.
- **`counts` per step** (`{}` for `tools` and a step that did not run):

  | Step | Counts |
  |---|---|
  | checkout | `files_checked` (the read files whose blob identity was checked) |
  | declarations | `selected`, `deleted`, `valid`, `excluded`, `read_files`, `read_bytes`, `found_after_first` |
  | pins | `files` |
  | admission | `packs`, `admitted` |
  | format | `packs`, `json_files`, `unformatted` (files) |
  | bundle | `packs`, `files` and `bytes` (the indexed packs' `file_count` and `byte_count`), `zip {name, sha256, entries}` (`null` when no zip was written), `root_files`, `package_revision` (3, or (Workflow #444 NOTICES-3) 10 when the bundle carries `notices/`) |

- **No prose.** Messages appear only on the CLI's human line and in developer lines on stderr.
  Failures and warnings are outcome envelopes with typed values. Paths are absolute and native for
  files on disk, and `/`-separated and repository-relative for repository content. Paths are data;
  nothing in the report names a game except as data read from the repository.
- **One schema per kind of run.** The wrapper accepts only the schema versions it knows, and
  requires every step the version defines to be present and `passed`.

## Codes

All in the actinglab fragment, `ladder_class` `none`, review `pending`, since 0.12.4:
`resource_verify_passed` (success, `step_count`), `resource_verify_failed` (`failed_step_count`),
`resource_verify_output_exists` (`path`), `resource_verify_git_failed` (`git_command`,
`exit_code?`; caused by `foreign_os_error` or `foreign_other_error` with git's stderr as
`raw_text`), `resource_verify_checkout_invalid` (`repo`, `checkout_problem`),
`resource_verify_checkout_dirty` (`path_count`, `first_path`, `dirty_problem`),
`resource_verify_base_invalid` (`base`, `base_problem`), `resource_verify_tools_mismatch`
(`manifest_path`, `tools_problem`), `resource_verify_layout_invalid` (`path`, `layout_problem`),
`resource_verify_pin_committed` (`path`, `json_pointer`, `member`),
`resource_verify_bundle_mismatch` (`path`, `bundle_problem`), `resource_verify_readback_mismatch`
(`entry`, `readback_problem`) and `resource_verify_report_write_failed` (`path`). The codes it
reuses carry the optional keys this slice declares on them: `resource_declaration_invalid`
(`path`, `json_pointer`, `found_after_first`, `step`), `package_invalid` (`path`, `step`),
`package_fmt_check_failed` (`path`, `unformatted`, `step`), and `step` on `validation_failed`,
`standard_pack_root_invalid` and the `package_bundle_*` errors.

`resource check-release` is retired: it answered `checked` without checking. It was refused with
`resource_check_release_retired` (exit 2), whose catalog text names `resource verify`; since
0.12.47 (Workflow #434 LAB-2) the word is gone, an unknown resource command, and the code is
retired.

Workflow #464 P2 (since 0.12.60), `standard-pack build`'s own codes, in the same fragment:
`resource_build_output_invalid` (`path`), `resource_build_failed`
(`failed_step_count`) and `resource_build_write_failed` (`path`).

## `standard-pack build`

Workflow #464 P2 (register [v012-register.md](v012-register.md) §1.376; the coordinator's rulings
on board #464, not the owner's words): one operation from a game's working root to the thing an
installation's placement accepts.

```text
actinglab [--json] standard-pack build --repo <checkout> --source-repository <owner>/<name>
    --out <directory> [--commit <40-hex>]
```

- **One function.** It runs the steps above, `tools` to `bundle`, every one of them, on the one
  function `resource verify` runs them on, its work under `<out>/work/` as `resource verify`'s is
  under `--work` (`packs-root/`, `bundle/`, `out/`, `read-back/`, `installed/`). `resource verify`
  runs them as before, so its report is the same for the same input. The build differs in five
  things only:
  - the checkout's read paths are every input its bundle carries: besides those of "Clean" above,
    `schedule.json`, `display-layout.json`, `loading.json`, `goals.json`, `ui.json` and every file
    under `loading/`, `notices/` and `tables/`, each the committed blob and none an ignored file;
  - `package bundle` is given `goals.json` too when the repository holds it (`--goals`);
  - the repository's tables, `tables/<table_id>.json` at its root, read as the Runtime reads a game
    root's tables (`read_standard_pack_tables`; a refused table fails the `bundle` step with its
    `standard_pack_table_invalid` or `standard_pack_table_unreadable` and is not carried), go into
    the zip as `tables/<file>`, byte for byte, and into the installed root, and the zip's
    `bundle.json` lists them in `tables`, in file name order (the index member an updater reads,
    [package-reference.md](package-reference.md)); without a table the zip's `bundle.json` is the
    one `package bundle` wrote;
  - (fix round 1) the repository's `ui.json`, a root file of the standard package root
    ([package-reference.md](package-reference.md), "Standard package root") that `place` places
    and sets aside when a zip carries none, goes into the zip's root byte for byte, and into the
    installed root, when its reader `parse_standard_pack_ui` takes it against the zip's
    `bundle.json`; each violation of a refused file fails the `bundle` step as
    `standard_pack_root_invalid` (`game`, `path`, `standard_pack_problem`, `json_pointer`) and the
    file is not carried. `package bundle` still has no flag for it: the build adds it to the zip,
    as it adds the tables;
  - `interfaces.json` declares the `package` revision the zip's content needs ("The `package`
    revision" below), not resource verify's 3 or 10.
- **Clean, and bound to its commit.** The `checkout` precondition refuses a tree with a change, an
  untracked file or an ignored file where the build reads, so every build has a commit and a tree.
  `--commit`, when given, must be HEAD. There is no `--base`.
- **`--out`** is a directory that does not exist, whose parent does, or an existing empty
  directory, and never `--repo` or a place inside it (by their real paths). Anything else is
  `resource_build_output_invalid` (`path`), before anything is read. Fix round 1: a `--out` that
  does not exist is made by the `bundle` step, the first step that writes, so only once the
  `checkout` precondition passed: a call it refuses (a tree with a change, for example) leaves
  nothing behind; one that cannot be made fails that step (`package_bundle_write_failed`, caused
  by the OS failure). A `--out` inside an installation root is not refused: no code of the Lab or
  the rescue module tells whether a directory lies inside one.
- **The product**, written only when every step passed: the steps' zip
  `<out>/<game>-resources-<sha7>.zip`, copied byte for byte from `<out>/work/out/`,
  `<out>/SHA256SUMS` (`<hex>  <name>\n`), and `<out>/build.json`. Fix round 1: each is written
  under its name with `.part` added, and only when all three are complete are they renamed, the
  zip, then `SHA256SUMS`, `build.json` last; a final name that exists is never replaced. A failure
  while they are written leaves no file under a final name, and `build.json` is there only beside
  the other two. For a commit whose repository holds no `goals.json`, no table and no `ui.json` and
  whose packs need no revision above 3, the zip is `resource verify`'s zip of that commit, byte for
  byte.
- **Steps that did not all pass** write no product: `resource_build_failed`
  (`failed_step_count`), caused by the steps' outcome (`resource_verify_failed` with each failed
  step's first failure, or the precondition's own code, `resource_verify_checkout_dirty` for a dirty
  tree); its details are `{outcome, report}`, `report` the steps' `actinglab.resource-verify.v1`
  document, which is no file. `--out` then holds what the steps wrote below `work/` and nothing
  else; when the precondition refused the call, a `--out` the build was to make does not exist. A
  file of the product that cannot be written or renamed is `resource_build_write_failed` (`path`,
  caused by the OS failure); the files already written stay.
- **The answer** (exit 0, the CLI module's line, its members flat): `build` (the document of
  `build.json`), `out`, `files` (`zip`, `sums`, `build`, `work`, absolute paths) and `steps` (each
  step as the report lists it).
- **Refusals** as `resource verify`'s: global `--dry-run` `dry_run_unsupported`, after flag parsing
  only, with no alternative; the device, Runtime and instance options
  `offline_device_scope_forbidden`; any other flag, a flag twice, a flag without its value, a
  positional, a `--commit` that is no 40 lowercase hexadecimal characters or a
  `--source-repository` that is no `<owner>/<name>` `validation_failed`.
- It writes nothing into the repository and deletes nothing. The MCP tool of its row is
  `ac_standard_pack_build` ([lab-operations.json](lab-operations.json)), served through a `serve`
  child. `capabilities` lists it with `build_schema: "actingcommand.standard-pack-build.v1"`.

### `build.json`

`actingcommand.standard-pack-build.v1`, exactly these members:

```json
{
  "schema_version": "actingcommand.standard-pack-build.v1",
  "game": "<the zip's bundle.json game>",
  "source_repository": "<owner>/<name>",
  "commit": "<40 hex: HEAD>",
  "tree": "<40 hex: HEAD^{tree}>",
  "tools": {"runtime_head": "<40 hex, compiled in>", "executable_sha256": "<64 hex>"},
  "package": [12, 12],
  "root_files": ["applications.json", "bundle.json", "goals.json", "interfaces.json"],
  "root_directories": ["notices"],
  "packs": [{"package_id": "…", "server": "…", "entry_task_id": "…", "file_count": 4, "byte_count": 1234}],
  "zip": {"name": "<game>-resources-<sha7>.zip", "sha256": "<64 hex>", "bytes": 0}
}
```

`game`, `package` (the range of the zip's `interfaces.json`), `root_files` (the files at the zip's
root, in byte order), `root_directories` (its directories but `packs`, in byte order) and `packs`
(its `bundle.json` packs in their order, without `path`) are read back from the zip written, whose
name, SHA-256 (the one `SHA256SUMS` names) and size `zip` gives; `commit` and `tree` are the
checkout's, `executable_sha256` the `tools` step's. It has no readiness member: whether a pack is
ready to be dispatched is judged by another slice's function, which a later slice adds here.

### The `package` revision

[component-interfaces.md](component-interfaces.md): "A bundle writes `package`: its `max` is the
revision its packs need. Give `min` the same value." The build declares `[r, r]`, `r` the highest
revision any entry of its zip needs, 3 (the bundle formats of Runtime v0.12.0) when none needs a
later one. The table holds what each `package` row of that document names and, fix round 1, what
the register sections it cites and the B train's sections put into a revision (their members
joined revision 6, "which BT-PK6 declares", and DB-2 declared 6 in 0.12.37), each in the file it
stands in and under its condition. The files of a pack: `control.json`; a task file
`resources/operations/<task>/task.json`; a selection-policy document
`resources/operations/<task>/policies/<name>.json`; the navigation projection below
`resources/navigation/`. "Under `x`": a member of an object that the member `x` holds, itself or
as an element of its array.

| Revision | File | What needs it | Source |
|---|---|---|---|
| 4 | task | `remedy` under `mappings` | B15, §1.110 B15-10, §1.111 |
| 4 | control | `provides` | B15 |
| 5 | task, navigation | a member `kind` that holds `single_touch_path_drag_v1` (a click) | T3, §1.137 |
| 6 | policy | `tables` of the document | DB-2, §1.65 TB6, §1.244 |
| 6 | policy | a member `source` that holds `table` | DB-2 |
| 6 | task | `post_admission_ocr` of a task of schema `0.9` only; a `0.7` or `0.8` task writes it and keeps its revision | OCR-SCALE-1, §1.251 OS-1 |
| 6 | task | `target_present`, `target_gone` under `expect_after` | ACT-4, §1.254 |
| 6 | task | `loading` under `expect_after` | LOAD-2, §1.258 LD2-1 |
| 6 | task | `wait_page` under `expect_after` | A35-S3, §1.176 S3-2; LOAD-2 LD2-2 |
| 6 | pack | its loading copy, `resources/loading.json` or a file under `resources/loading/` | LOAD-1, §1.257 LD1-2 |
| 6 | policy | a member `kind` that holds `integer_compare` | BT-B2, §1.146 B2-1 |
| 6 | task | `part` under `integer` (an `ocr_integer` feature) | BT-B2 B2-2 |
| 6 | task | `alternatives` under `color_probes` or under `color_check` | BT-B3, §1.149 |
| 6 | task | `busy` under `mappings` | A35-S1, §1.152 B37-1 |
| 6 | task, control | `end_pages`, `entry_pages` under `phases` | BT-B5, §1.154 B5-1, B14-1 |
| 6 | task | a member `kind` that holds `still_frames` under `input_recheck` | BT-B12, §1.155 B12-1 |
| 6 | task | `input_recheck` under `operations`, in a pack whose `control.json` `execution_mode` is `linear_steps` only | BT-B12 B12-4; resource-declarations.md, "until then it, and an input recheck under `linear_steps`, are admitted by code" |
| 6 | task | `dictionary` under `value`, in a value whose `type` is `unsigned_integer` only (the `dictionary` of a `dictionary_entry` value is older) | BT-B1, §1.163 B1-1 |
| 6 | task | `agreement` under `left`, `right` (a value-test side) or `resource_readings` (a reading mark) | BT-B1 B1-2 |
| 6 | task | `max_runs`, `on_max_runs` under `operations`; `on_guard_refused` under `operations` when it holds an object (the text `next_candidate` is older) | BT-KEY, §1.164 KEY-1, KEY-2 |
| 6 | task | `luma_floor` under `anchors` or `verify_templates` | BT-B13, §1.165 B13-1 |
| 6 | task | `confirm` under `select`, whatever it holds | BT-B4, §1.166 B4-1 |
| 6 | task | `wait` under `operations` | A35-S3, §1.176 S3-1 |
| 6 | task | a member `type` that holds `timestamp_ms` under `value` | CF-2, §1.181 CF2-1 |
| 6 | root `interrupts.json` | `limit_cause`, `result` under `interrupts` | B43, §1.203 B43-1; UPD-1a, §1.215 UPD-1 |
| 6 | control | `store_update` | STORE-FG-1, §1.207 SF-1 |
| 6 | task | `preprocess` under `ocr_targets` or `shared_ocr` | CFL-2, §1.208 CFL2-1 |
| 7 | task | a member `value` that holds `anchor_milli` under `features` | FS-2, §1.283 FS2-1 |
| 7 | task | a member `confirm` that holds `track`, and `track_px`, under `select` | FS-3, §1.284 FS3-1 |
| 7 | task | a `repeated_anchor` layout of `candidate_layouts` whose `anchor` names a target of the task's `anchors` that declares `mask`, `maskRange` or `scales` | FS-1, §1.282 FS1-3 |
| 8 | task | `read_list` under `operations` | S168-4, §1.291 S168C-1 |
| 8 | policy | a member `source` that holds `table_max_over_list` | S168-5a, §1.291 S168C-5 |
| 9 | policy | `lower_bound_column` | S168-LB1b, §1.309 LB1-7, §1.312 |
| 10 | root | a file under `notices/` | NOTICES-2, §1.335 |
| 11 | task | `field_lists` of the task | FL-1, §1.343 |
| 12 | root | `goals.json` | G-R1, §1.356 |

Nothing else needs a revision: a game root's `tables/` (row 6: no member of a pack reads them),
`ui.json` (§1.240: no revision), the other root files and every other file of a pack need 3. A
member is found by its name, its file and its place, not by the declaration reader; the generic
names are held to their place and, where an older member of the same name exists, to the value
or the condition that tells them apart, so the declaration is neither lower nor higher than the
rows name. A JSON file that does not read needs nothing here (the `format` step refuses it). A
feature that adds a `package` revision adds its row, in its own slice
(`apps/actinglab/src/resource_verify/revision.rs`). `resource verify` keeps its rule: 3, or 10
with `notices/`; the two rows can therefore declare different revisions for one commit (a follow-up
slice on `resource verify`).

## What stays in the wrapper

The repository wrapper keeps what is the repository's policy, not a Runtime rule:

- the Tools pin (the release download, the archive SHA-256, `BUILD-MANIFEST.json` and the files
  beside the executable): the pin is the repository's Actions configuration and needs the network;
  a binary cannot attest that it is the pinned one, so the command reports its compiled head and
  its executable's SHA-256 for the wrapper to compare;
- the clean detached worktree (where temporary trees live, LFS and disk rules): the command only
  proves the tree is clean and at the commit;
- the commit author and committer allowlist (personal e-mail addresses; its range depends on pull
  request or push);
- the report index, the pre-push hook and "the base is contained in the remote main" (git hooks
  and the merge policy).

## Versioning against the pin

- **What versions the command:** its capability entry (`capabilities` lists `resource verify`
  with `report_schema: "actinglab.resource-verify.v1"`), the report's `schema_version`, and the
  regression case contract's `schema_version`. The first Tools release that ships the command and
  the cases a repository needs is that repository's minimum pin.
- **A pin that predates the command.** The wrapper verifies the local Tools against the pin first,
  then reads `capabilities`. With no `resource verify`, or an unknown `report_schema`, it stops and
  names the minimum release; it never falls back to earlier steps silently. A repository keeps its
  earlier validation until its own pin bump.
- **A pin that lags the deployed Runtime.** The repositories are then checked by older rules than
  production applies. After each Runtime deployment the three repositories bump the Tools pin to
  that release, with a passing `verify`, so the pin never lags production by more than one
  deployment (coordinator decision D5, 2026-10-19).
- **A newer verify, older cases.** An unknown case `schema_version` fails as a case refusal; it is
  never skipped.
