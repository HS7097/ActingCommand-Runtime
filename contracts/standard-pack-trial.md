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
actinglab standard-pack trial <instance> --package <dir> [--authorization <reference>]
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

**A plan prepares and shows** (the authorization line's first kind, "prepare and show"; the
coordinator's ruling of fix round 2): it needs no authorization and refuses nothing that only the
run needs. What the run will require that the plan was not given, the plan says
(`run_requires`); the refusals that belong to the run (an authorization missing, an irreversible
package not acknowledged) come with the slice that sends it.

| Input | What it is |
|---|---|
| `<instance>` | The instance the trial is for. The plan shows it as given; it is not contacted, resolved or read. |
| `--package` | The package directory tried ("The package"). |
| `--authorization` | The reference of the owner's authorization ("The authorization"); optional for a plan, required by the run. |
| `--variable` | A value of a variable the package declares, `<name>=<json>`, once per name ("The variables"). |
| `--expect` | The outcome key the trial is expected to end with: one the package declares. |
| `--irreversible` | Acknowledges a package the irreversible mark flags ("The irreversible mark"); shown in the plan. |
| `--root` | A game root, installed or a bundle laid out as one, its directory named as its game: the root the irreversible mark is judged over. A package it does not register counts as marked. |
| `--state-dir` | The Lab's state directory, as the recording commands take it (default actinglab's session directory), where trial records are kept ("The record"). |
| `--plan` | Answer the plan only; required in this release. |

**What the plan reads**: the package directory, once, and with `--root` once more for the
irreversible mark ("The irreversible mark"); the game root `--root` names; and whether the entries
of "The state directory's place" lie along the state directory's path. No Runtime, no device, no
installation; it writes nothing (the state directory is not made) and sends nothing.
Under the global `--dry-run` it answers the same ([actinglab-dry-run.md](actinglab-dry-run.md),
`no_effect`).

**Malformed arguments** are `validation_failed` naming the flag (`argument`, `argument_problem`
and `arguments`), as the command line answers every other: one code for one kind of mistake.

## The package

**Only a directory is tried**: `packs/<id>` of a checkout, or a placed pack
`<standard_packs.root>/<game>/<id>/`. A file (a `.zip` or `.json` container, or any other) is
`validation_failed` naming `--package` with `argument_problem` `unusable`, the vocabulary's word
for an argument that names something not of its kind (no word of the command line says "a path of
the wrong kind" more closely), and the message says why a directory is required: adoption rewrites
the package's JSON into the canonical form
([standard-pack-authoring.md](standard-pack-authoring.md)), so a container and the directory
adopted from it may hash differently, and what is tried must be what was adopted. A path that does
not exist, and a directory the Runtime's loader does not admit, are `package_invalid`.

**One reading.** The directory is read once: one bounded snapshot by the containment crate's own
reader (`snapshot_content_directory`), and every member of the plan but the irreversible mark is
derived from those bytes, so that all of them describe the same content. The kernel admits the
same bytes (`PreparedContainedTask::load_content_entries`, offline without a vision provider: no
model is required); the ends and the variables are read from the same snapshot. The irreversible
mark is judged by the functions every other reader of a root calls, never by a copy of them (fix
round 3), and the reader they call of a package's declarations
(`PreparedContainedTask::read_remedy_declarations`) takes only a path: over a root the directory
tried is therefore read once more, by that reader, for the mark alone.

**The content** is the hash of those bytes: the contract crate's `content_directory_digest` of each
file's SHA-256, exactly what the containment crate's `measure_content_hash` computes for a
directory, the function a scheduled run's failure identity and its suspension's lift measure a
package with. The plan keeps its first 12 hex digits, `content_hash_prefix`: the digits the failure
identity keeps, and the digits the break-in of a package's content keeps (pull request #1737,
which measures with the same function and takes the same 12 digits). So a record and a break-in
name the same content by the same prefix.

## The authorization

`--authorization <reference>`: a text of 1 to 1024 bytes without a control character, the
reference of the owner's word (the board comment that holds it, for example). The tool records it
and shows it back; it never interprets it, and a reference is no permission check. A plan needs
none: without it the plan answers `authorization` `null` and `run_requires` `["authorization"]`,
the run will require one. Given, it is checked: given without a value, or blank,
`validation_failed` naming `--authorization` with `value_missing`, as every option without its
value; longer than 1024 bytes `out_of_range`; a control character `not_recognised`; each before
anything is read.

## The variables

Each variable the package declares (`control.json` `variables`, [package-variables.md](package-variables.md))
is listed in declaration order with the value the trial runs with: the value `--variable` gives
(`source` `given`) or its declared default (`default`). `--variable` is parsed by the scan replay's
own parser (one function): a flag without a value is `value_missing`, a value that is not
`<name>=<json>` (no `=`, an empty name, a right side that is no JSON) `not_recognised`, a name given
twice `repeated`. A given value is judged **offline** against the package's declaration by the
contract crate's `PackageVariable::judge`, the rule a dispatch resolves a value by, and must be
taken exactly as given: a name the package does not declare, and a value the declaration does not
take or would take in part, are `validation_failed` naming `--variable` with `not_recognised`, the
details carrying `variable_name` and, for a value, the judgement's word `variable_problem`
(`not_an_option`, `out_of_range`, `off_step`, `type_changed`, `keys_removed`,
`count_out_of_range`, `key_repeated`, or `keys_left_out` for a value taken in part). Only the
package's declaration judges a trial's value; what a game root keeps from applying is a dispatch's
matter.

## The irreversible mark

With `--root`, the package is judged by the contract crate's own `IrreversiblePackages` over that
root ([standard-pack-schedule.md](standard-pack-schedule.md), "The irreversible mark"), as the
resource tooling's own `root_irreversibility` builds it for the goal check and the composer, called
and not copied: every package the root's `bundle.json` registers, each read by the Runtime's
metadata reader at its registered directory, the registered directory of the id tried read at the
directory tried in its place. The plan answers `irreversible`
`marked` or `unmarked` and `irreversible_basis`: the `root`, whether it `registered` the package,
the `capabilities` a package of the root marks irreversible that the package provides or its
remedies list, and the root's `unreadable_packages`. The type's safe side holds: a package the
root does not register, and every package while one of the root cannot be read, counts as marked.
`irreversible_acknowledged` says whether `--irreversible` was given. **The plan shows the mark and
refuses nothing for it** (fix round 2); a marked package that is not acknowledged is the run's to
refuse.

**Without `--root` the mark cannot be judged**: the plan answers `irreversible` `not_judged`. This
is a limit of this release; S3-11 decides whether the run requires `--root`.

## The plan

The answer of `--plan`:

| Member | What it is |
|---|---|
| `sent` | `false`: nothing was sent. |
| `plan.package_id` | The package's id, as its control declaration states it. |
| `plan.content_hash_prefix` | "The package". |
| `plan.package` | The directory tried, absolute. |
| `plan.instance` | The instance, as given. |
| `plan.authorization` | The reference, as given; `null` when none was given. |
| `plan.ends` | Each `scheduling_outcome` mapping of the entry task, in mapping order: `outcome_key`, `end` and `pages`, its terminal pages as the task writes them. `end` names each kind by the declaration that makes it one, as the package's own contracts name them, told apart in this order: `target` (its terminal pages hold a target page of the task, the kernel's own target set), `on_empty` (a select step's `on_empty` key, [selection-graph.md](selection-graph.md)), `on_max_runs` and `on_guard_refused` (the key of an operation's keyed `on_max_runs` or keyed `on_guard_refused`, [resource-declarations.md](resource-declarations.md), "Per-run operation caps and keyed guard refusals"), `on_timeout` (a wait step's `on_timeout`, "Wait steps and wait pages"), `end_page` (a page a stage lists in its `end_pages`, [README.md](README.md), "Stage end pages and entry pages"), and for every other mapping `ending` in a `linear_steps` task (an ending page, [linear-steps.md](linear-steps.md), "Endings") or `other` in a page graph. |
| `plan.variables` | "The variables": each `{name, value, source}`. |
| `plan.irreversible`, `plan.irreversible_basis`, `plan.irreversible_acknowledged` | "The irreversible mark"; the basis is absent when the mark was not judged. |
| `plan.expect` | `--expect`, when given; a key the package does not declare is `validation_failed` naming `--expect` with `not_recognised`, the word an undeclared `--variable` name gets, the details carrying the key given as `expected` and the declared keys as `declared_outcome_keys`. |
| `run_requires` | What the run will require that this plan was not given: `authorization` when `--authorization` was not given; empty otherwise. |
| `record_directory` | Where the trial's record is to be kept: `<state>/trials/<package_id>`. |

The refusals come in this order: the command line's own (a flag the row does not take, a word
beyond it, a flag given twice, `--plan`, `--instance` and `--package` absent, an option without its
value, `--variable`), then the authorization's bounds, the package path, the state directory's
place, the package's reading and admission, the variables' values, `--expect`, and last `--root`'s
own: a root that is no directory, or whose directory name is no game's (`validation_failed`); a
root file that cannot be read, or is larger than the package input limit (`validation_failed`);
a root without `bundle.json`, or a root file that breaks its shape, a limit or a reference
(`standard_pack_root_invalid`).

## The record

`actingcommand.pack-trial.v1`, one JSON document per trial, at
`<state>/trials/<package_id>/<content12>-<ms>.json`, `<content12>` the plan's
`content_hash_prefix` and `<ms>` the time the trial ended. It is Lab authoring state beside the
recordings: never in a repository, never in an installed root.

| Member | What it is |
|---|---|
| `schema_version` | `actingcommand.pack-trial.v1`. |
| `at_unix_ms` | When the trial ended; it names the file. |
| `plan` | The plan the trial ran by, as `--plan` answers it, but its `authorization` is the reference the run was sent under, never `null` (fix round 3): the record's plan is a type of its own whose `authorization` is a text, made from a plan only by a conversion that refuses a plan without one (`validation_failed`, as the writer refuses another malformed record), and the reader decodes into it, so a stored record whose `authorization` is absent or `null` is `trial_record_unreadable` `undecodable`. |
| `request_id`, `run_id` | The run's request id, and its run id when it has one. |
| `end` | How the run ended, as its terminal says it: `outcome` (`success`, `failure`, `cancelled`), `outcome_key` (the business end), `final_page`, `failure_code`, `failure_values`, each when it has one. |
| `matched` | Whether `end.outcome_key` is `plan.expect`; absent without `--expect`. |

**The state directory's place.** A state directory that is, or lies inside, an installed root or a
repository is refused `trial_state_dir_refused` with its `path` and the word `trial_state_place`:
`installed_root` when a directory along its path (made absolute, and again with its nearest
existing part's links resolved) has `install\state.json` as a file, the installer's own state
file in both layouts ([install-cli.md](install-cli.md), "Layout"); `repository` when one holds
`.git`. The installed root is recognised by one function of the resource tooling,
`installed_root_containing`, below the Lab's crate so that both can call it; the Runtime host's
`installation_root_at` recognises an installation root of one directory by the same file, but the
resource tooling may not reach the host's crate (an architecture guard), so the function repeats
only the file's name and walks the path itself. A `runtime\BUILD-MANIFEST.json` (in every unpacked
release and kept build, a member folder's mark) or an `install\active.json` alone makes no
installed root. A check the operating system refuses (a part of the path whose existence or kind
it does not tell, or that cannot be resolved) is `foreign_os_error` with that `path` and its
`io_op`, never read as "absent". The plan and the writer both refuse it, before anything is
written.

**What the repository refusal costs**: a state directory kept inside a checkout (a resource
repository's, for one) is refused for the trial's records, though the recordings accept it there;
`--state-dir` names another, outside every repository and installation.

**The writer** (`write_trial_record`) writes a record once: whole, and flushed, under its name
with `.part` added, a file created anew, then hard-linked to its name, which adds that name in one
operation and fails when the name is taken (a rename would replace an existing file on Windows).
A record of that name, or a `.part` of it, that is there already is `trial_record_exists` naming
that file, and is never replaced; nothing is built from a forged error. When the link (or the
exclusive create below) finds the record there, the `.part` this call wrote, holding the record it
was given, is left beside it. It takes only a record whose plan holds an authorization (the
record's plan type, above), and refuses another malformed record (a `schema_version` other than
this one, a `package_id` that names no directory, a `content_hash_prefix` that is not 12 hex
digits) `validation_failed`.

**Where no hard link can be made** (fix round 3; FAT32, exFAT, some network shares): a link the
operating system refuses for another reason than the name being taken falls back to an exclusive
create of the record's own name, which fails when the name is taken, the bytes written whole and
flushed, as the Runtime host places its installation identity ([install-cli.md](install-cli.md),
"Layout", `install\identity.json`). What it keeps of "never replaced": no file of that name is
ever written over, and a record there is `trial_record_exists` as above. What it gives up against
the link: the name is there before its bytes are, so a reader at that moment, or a crash while they
are written, finds an incomplete record, which the reader refuses (`undecodable`) until the write
under way completes or a person moves a crash's leftover away.

**A failed step** (fix round 3): a step the operating system refuses (making the directory,
creating the `.part`, writing it, flushing it, which is `io_op` `sync`; where it stands in for the
link, the exclusive create, writing or flushing that file, with the link's refusal as a `note`
link) is `trial_record_write_failed`, its refusal the cause, and no record is written. The files
this call created anew, made with `create_new` and nothing else's (its `.part`, and the record's
file of an exclusive create), are then removed, so that writing the same record again is not
blocked by this call's own leftovers and the answer names the real failure. One whose removal the
operating system refuses too is left: the answer is still the failure, naming the file left by a
`cleanup` link (`foreign_os_error`, `io_op` `remove`, `path` that file) and in its message.

**Once the record is in place** the writer removes its `.part`. A removal the operating system
refuses then does not make the write a failure, since the record is written: the writer answers
success, the record's `path` with `part_left` naming the `.part` left and, in `warnings`, the
warning `trial_record_part_left` (`path` the `.part`, the refusal its cause), in the shape every
Lab answer carries warnings in. In this release nothing writes a record outside the tests.

**The reader** answers every record of a package (`trial_records`), in the order of their times
(then by content), and the latest record of one content of a package (`latest_trial_record`); none
when there is none. A file of a record's name that is not read as the record its name says is
`trial_record_unreadable` (`path`) with the required word `trial_record_problem`: `read_failed`
(the operating system refused to open, read or list it; its refusal as the cause), `too_large`
(larger than 1 MiB), `undecodable` (no such document, a plan without an authorization among them;
the decoder's refusal as the cause) or `name_mismatch` (a record, but not of the package, the
content and the time its name says): the reader answers no record of that package rather than pass
over one. A file of another name, a `.part` among them, is no record. The status read (S3-14) and
adopt's review (S3-12) are to call it.

## Outcome codes

Each refuses (category `error`, class `none`) but `trial_record_part_left`: exit 1 on actinglab's
command line, which prints the CLI module's line, and exit 2 in the envelope a `--via` line
prints. `trial_record_part_left` warns (category `warning`, class `none`, fix round 3): it rides in
the `warnings` of a successful write and leaves its exit at 0. The command line's own refusals and
the malformed arguments are `validation_failed`, as above; a check of the state directory's path
the operating system refuses is `foreign_os_error`.

| Code | The one situation | Refuses / warns |
|---|---|---|
| `trial_state_dir_refused` | The state directory is, or lies inside, an installed root or a repository (`trial_state_place`). | refuses |
| `trial_record_exists` | A record or its `.part` of that name is already there: it is never replaced; a person looks at that file. | refuses |
| `trial_record_write_failed` | The operating system refused a step of the write, and no record was written. | refuses |
| `trial_record_unreadable` | A file of a record's name is not that record (`trial_record_problem`). | refuses |
| `trial_record_part_left` | The record is written and in place, but the `.part` its write created could not be removed. | warns |
