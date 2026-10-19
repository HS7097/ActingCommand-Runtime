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
    files `pages.json`, `interrupts.json` and `navigation.json` when present;
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
- **Result.**
  - **Exit 0:** `ok: true`, and the data is the report.
  - **Exit 2, a failed step:** `resource_verify_failed`. `details` is the report; its outcome's
    causes are each failed step's first failure, with `step`.
  - **Exit 2, a precondition:** `resource_verify_checkout_invalid`,
    `resource_verify_checkout_dirty`, `resource_verify_base_invalid` or
    `resource_verify_git_failed`, with the report in `details`.
  - **Exit 2, a usage error before anything is read:** `validation_failed` or
    `resource_verify_output_exists`, and no report.
  - **A report that cannot be written:** `resource_verify_report_write_failed` (exit 2). stdout
    still carries the whole result: `details` is the report.
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
| `bundle` | `bundle.ps1:189-465` with the root files and the installed root, under `--work`: `packs-root/` (the pack directories staged), `bundle/` (`package bundle` with `applications.json`, `maintenance.json`, the source and the root files present), the index and copy checks (`resource_verify_bundle_mismatch`), `out/<game>-resources-<sha7>.zip` (entries in ordinal order, `/`-separated, with `interfaces.json` `{schema_version actingcommand.component-interfaces.v1, speaks {package [3, 3]}, validated_with {repository HS7097/ActingCommand-Runtime, commit <runtime_head>}}` beside `bundle.json`, LF-terminated) and `out/SHA256SUMS` (`<hex>  <name>\n`), `read-back/<game>/` (the same entries and bytes, and `interfaces.json` as written: `resource_verify_readback_mismatch`), `installed/<game>/` (the root files beside `bundle.json`, each `packs/<id>/` as `<id>/`) checked by `standard-pack check` (its `root_files` must be `bundle.json` and the root files given; its warnings `package_bundle_entry_page_unlisted` and `package_bundle_remedy_invalid` are carried), and every read-back pack admitted again, stating the indexed `package_id`, server and entry task. | `actingcommand_resource_tooling::package_bundle`, `check_standard_pack_root`, `package_cli::preflight_admission`, `zip` | for a release script that checked the zip layout, it replaces that check by the installed root the Runtime's registry reads |

The index and copy checks (`resource_verify_bundle_mismatch`, its `bundle_problem`):
`index_schema` (bundle.json is no bundle v4 index); `maintenance_differs` (the maintenance read
back, from the answer and from `bundle.json`, differs from `maintenance.json`, order of members
aside); `root_file_differs` (the bundle root holds other entries than `applications.json`,
`bundle.json`, `packs` and the root files given, or a root file differs from its source);
`applications_differs` (the bundled `applications.json` bytes, the index's game, or a game that is
no directory name); `source_differs`; `pack_list_differs` (the indexed packs are not the pack
directories, or a pack's path, server or entry task differs from its source); `pack_copy_differs`
(a copy is not its source, file for file and byte for byte); `pack_counts_differ` (`file_count`
or `byte_count`); `default_package_unmatched` (a server's `default_package_id` names not exactly
one bundled pack of that server); `root_files_unread` (`standard-pack check` of the installed
root read other root files, another number of packs or another game). A pack is copied only as
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
  | bundle | `packs`, `files` and `bytes` (the indexed packs' `file_count` and `byte_count`), `zip {name, sha256, entries}` (`null` when no zip was written), `root_files`, `package_revision` |

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

`resource check-release` is retired: it answered `checked` without checking. It is refused with
`resource_check_release_retired` (exit 2), whose catalog text names `resource verify`.

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
