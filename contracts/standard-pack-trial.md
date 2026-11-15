# Standard-pack trial: `standard-pack trial`

Workflow #463, slice S3-10 (Runtime 0.12.63; [v012-register.md](v012-register.md) §1.417). A
package that has not run through on a real device stays held, and packages are opened one by one
as each is run through; "run through" is one real run on a real application, and a trial on a real
application is an act under the owner's explicit authorization. `actinglab standard-pack trial`
is the tool of that one run. This slice builds the half that shows and refuses and keeps the
record: it answers the plan of the run and every refusal before it, and defines the trial record
with its writer and its reader. **It sends nothing.** The run, and the record it leaves, are the
next slice's (S3-11), which builds on this one.

Three sentences every text of the tool keeps true:

- **A passed trial is not an authorization.** The record says which content was tried, under which
  reference, and how its run ended; it lifts no hold, places nothing and makes no package enter
  scheduling. Enabling a package is the placement's act, under its own authorization.
- **A trial counts nothing toward a break-in.** A trial is a Lab run: it belongs to no pair of
  [policy-suspension.md](policy-suspension.md), and the break-in of a package's content (pull
  request #1737) counts only the settled runs of a pair in scheduling.
- **A trial is one run, sent once, never retried by the tool.** Nothing starts one by itself: no
  trial follows an adoption, a build or a placement, and the tool runs no loop and changes no hold.

## The operation

```text
actinglab standard-pack trial <instance> --package <dir> --authorization <reference>
    [--variable <name>=<json>]... [--expect <outcome_key>] [--irreversible]
    [--root <game root>] [--state-dir <dir>] --plan
```

The row `standard-pack trial` of the operation table ([lab-operations.json](lab-operations.json),
module `authoring`, slice `PACK-TRIAL`); over MCP the tool `ac_standard_pack_trial`, served by a
`serve` child ([lab-two-entries.md](lab-two-entries.md)). On actinglab's command line the instance
is written `--instance <x>`, as on every own command that acts on one.

**In this release the row answers only with `--plan`.** Without it the line is refused before
anything is read: `validation_failed` naming `--plan` with `argument_problem` `required`, the word
the command line gives a required flag that is missing. S3-11 lifts this refusal when it builds the
run.

| Input | What it is |
|---|---|
| `<instance>` | The instance the trial is for. The plan shows it as given; it is not contacted, resolved or read. |
| `--package` | The package directory tried ("The package"). |
| `--authorization` | The reference of the owner's authorization ("The authorization"). |
| `--variable` | A value of a variable the package declares, `<name>=<json>`, once per name ("The variables"). |
| `--expect` | The outcome key the trial is expected to end with: one the package declares. |
| `--irreversible` | Acknowledges a package the irreversible mark flags ("The irreversible mark"). |
| `--root` | A game root, installed or a bundle laid out as one, its directory named as its game: the root the irreversible mark is judged over. |
| `--state-dir` | The Lab's state directory, as the recording commands take it (default actinglab's session directory), where trial records are kept ("The record"). |
| `--plan` | Answer the plan only; required in this release. |

**What the plan reads**: the package directory, the game root `--root` names and whether the
markers of "The record" lie along the state directory's path. No Runtime, no device, no
installation; it writes nothing (the state directory is not made) and sends nothing. Under the
global `--dry-run` it answers the same ([actinglab-dry-run.md](actinglab-dry-run.md), `no_effect`).

## The package

**Only a directory is tried**: `packs/<id>` of a checkout, or a placed pack
`<standard_packs.root>/<game>/<id>/`. A file (a `.zip` or `.json` container, or any other) is
refused `trial_package_not_directory` with its `path`: adoption rewrites the package's JSON into the
canonical form ([standard-pack-authoring.md](standard-pack-authoring.md)), so a container and the
directory adopted from it may hash differently, and what is tried must be what is adopted. A path
that does not exist, and a directory the Runtime's loader does not admit, are `package_invalid`.

**The content** is the hash of the directory given, measured by the containment crate's own
`measure_content_hash`, the function a scheduled run's failure identity and its suspension's lift
measure a package with. The plan keeps its first 12 hex digits, `content_hash_prefix`: the digits
the failure identity keeps, and the digits the break-in of a package's content keeps (pull request
#1737, which measures with the same function and takes the same 12 digits). So a record and a
break-in name the same content by the same prefix.

## The authorization

`--authorization <reference>`: a text of 1 to 1024 bytes without a control character, the
reference of the owner's word (the board comment that holds it, for example). The tool records it
and shows it back; it never interprets it, and a reference is no permission check. Absent or blank
is `trial_authorization_missing`, before anything is read; longer than 1024 bytes is
`validation_failed` naming `--authorization` with `out_of_range`; a control character
`not_recognised`. An `--authorization` on the command line without a value names none.

## The variables

Each variable the package declares (`control.json` `variables`, [package-variables.md](package-variables.md))
is listed in declaration order with the value the trial runs with: the value `--variable` gives
(`source` `given`) or its declared default (`default`). `--variable` is parsed as the scan
replay's: a flag without a value is `value_missing`, a value that is not `<name>=<json>` (no `=`, an
empty name, a right side that is no JSON) `not_recognised`, a name given twice `repeated`. A given
value is judged **offline** against the package's declaration by the contract crate's
`PackageVariable::judge`, the rule a dispatch resolves a value by, and must be taken exactly as
given: a name the package does not declare, and a value the declaration does not take or would
take in part, are `validation_failed` naming `--variable` with `not_recognised`, the details
carrying `variable_name` and, for a value, the judgement's word `variable_problem`
(`not_an_option`, `out_of_range`, `off_step`, `type_changed`, `keys_removed`,
`count_out_of_range`, `key_repeated`, or `keys_left_out` for a value taken in part). Only the
package's declaration judges a trial's value; what a game root keeps from applying is a dispatch's
matter.

## The irreversible mark

With `--root`, the package is judged by the contract crate's own `IrreversiblePackages` over that
root ([standard-pack-schedule.md](standard-pack-schedule.md), "The irreversible mark"): every
package the root's `bundle.json` registers, read by the reader the goal check and the composer use,
the directory tried read in place of the registered directory of its id. The plan answers
`irreversible` `marked` or `unmarked` and `irreversible_basis`: the `root`, whether it `registered`
the package, the `capabilities` a package of the root marks irreversible that the package provides
or its remedies list, and the root's `unreadable_packages`. The type's safe side holds: a package
the root does not register, and every package while one of the root cannot be read, counts as
marked. A marked package without `--irreversible` is refused `trial_irreversible_unacknowledged`
(`package_id`, `path` the root).

**Without `--root` the mark cannot be judged**: the plan answers `irreversible` `not_judged`, and
`trial_irreversible_unacknowledged` cannot happen. This is a limit of this release; S3-11 decides
whether the run requires `--root`.

## The plan

The answer of `--plan`:

| Member | What it is |
|---|---|
| `sent` | `false`: nothing was sent. |
| `plan.package_id` | The package's id, as its control declaration states it. |
| `plan.content_hash_prefix` | "The package". |
| `plan.package` | The directory tried, absolute. |
| `plan.instance` | The instance, as given. |
| `plan.authorization` | The reference, as given. |
| `plan.ends` | Each `scheduling_outcome` mapping of the entry task, in mapping order: `outcome_key`, `end` (`target` when its terminal pages hold the task's target page, `on_empty` for a select step's `on_empty` key, `ending` for every other, an ending page) and `pages`, its terminal pages as the task writes them. |
| `plan.variables` | "The variables": each `{name, value, source}`. |
| `plan.irreversible`, `plan.irreversible_basis`, `plan.irreversible_acknowledged` | "The irreversible mark"; the basis is absent when the mark was not judged. |
| `plan.expect` | `--expect`, when given; a key the package does not declare is refused `trial_expectation_unknown` (`package_id`; the details carry the key given as `expected` and the declared keys as `declared_outcome_keys`). |
| `record_directory` | Where the trial's record is to be kept: `<state>/trials/<package_id>`. |

The refusals come in this order: the command line's own (a flag the row does not take, a word
beyond it, a flag given twice, `--plan`, `--instance` and `--package` absent, an option without its
value, `--variable`), then the authorization, the package path, the state directory's place, the
package's admission, the variables' values, `--expect`, the irreversible mark.

## The record

`actingcommand.pack-trial.v1`, one JSON document per trial, at
`<state>/trials/<package_id>/<content12>-<ms>.json`, `<content12>` the plan's
`content_hash_prefix` and `<ms>` the time the trial ended. It is Lab authoring state beside the
recordings: never in a repository, never in an installed root.

| Member | What it is |
|---|---|
| `schema_version` | `actingcommand.pack-trial.v1`. |
| `at_unix_ms` | When the trial ended; it names the file. |
| `plan` | The plan the trial ran by, as `--plan` answers it. |
| `request_id`, `run_id` | The run's request id, and its run id when it has one. |
| `end` | How the run ended, as its terminal says it: `outcome` (`success`, `failure`, `cancelled`), `outcome_key` (the business end), `final_page`, `failure_code`, `failure_values`, each when it has one. |
| `matched` | Whether `end.outcome_key` is `plan.expect`; absent without `--expect`. |

**The state directory's place.** A state directory that is, or lies inside, a repository or an
installed root is refused `trial_state_dir_refused` with its `path` and the word
`trial_state_place`: `repository` when a directory along its path (made absolute, and again with its
nearest existing part's links resolved) holds `.git`, `installed_root` when one holds the markers
by which the Lab's location finds an installed root (`runtime\BUILD-MANIFEST.json` of a
single-location root, `install\active.json` of a root with a selection). The plan and the writer
both refuse it, before anything is written. Main had no test of whether a directory lies inside
either; the Lab's location finds an installed root by the place of a running program, and this
test asks the same markers of the state directory's path.

**The writer** (`write_trial_record`) writes a record once: whole under its name with `.part`
added, then renamed into place. A record of that name, or a `.part` of it, that is there already is
never replaced: `trial_record_write_failed` (`path`, the operating system's refusal or
`AlreadyExists` as its cause), as is every write that fails. Nothing is removed. In this release
nothing writes a record outside the tests.

**The reader** answers every record of a package (`trial_records`), in the order of their times
(then by content), and the latest record of one content of a package (`latest_trial_record`); none
when there is none. A file of a record's name that cannot be read, is larger than 1 MiB, does not
decode as the document, or is not the record of the package, the content and the time its name
says is `trial_record_unreadable` (`path`, with the read's or the decoder's refusal as its cause
where there is one): the reader answers no record of that package rather than pass over one. A file
of another name, a `.part` among them, is no record. The status read (S3-14) and adopt's review
(S3-12) are to call it.

## Outcome codes

Each refuses (category `error`, class `none`): exit 1 on actinglab's command line, which prints
the CLI module's line, and exit 2 in the envelope a `--via` line prints; none warns. The command
line's own refusals are `validation_failed`, as above.

| Code | The one situation |
|---|---|
| `trial_authorization_missing` | `--authorization` absent or blank. |
| `trial_package_not_directory` | `--package` names a file, not a directory. |
| `trial_irreversible_unacknowledged` | The root marks the package and `--irreversible` is not given. |
| `trial_expectation_unknown` | `--expect` names no key the package declares. |
| `trial_state_dir_refused` | The state directory is, or lies inside, a repository or an installed root (`trial_state_place`). |
| `trial_record_write_failed` | A record could not be written, or one of its name is there. |
| `trial_record_unreadable` | A file of a record's name is no such record. |
