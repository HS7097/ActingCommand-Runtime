# Standard-pack trial: `standard-pack trial`

Workflow #463, slice S3-10 (Runtime 0.12.64; [v012-register.md](v012-register.md) §1.417). A
package that has not run through on a real device stays held, and packages are opened one by one
as each is run through; "run through" is one real run on a real application, and a trial on a real
application is an act under the owner's explicit authorization. `actinglab standard-pack trial`
is the tool of that one run. S3-10 built the half that shows and refuses and keeps the record: it
answers the plan of the run and every refusal before it, and defines the trial record with its
writer and its reader; a plan sends nothing. Slice S3-11 (Runtime 0.12.65; register §1.418) builds
the run: without `--plan` the line sends the trial's one run, reads how its business ended and
records it ("The run", "The end", "Reading the end later"). The tool records the authorization
reference it is given and judges nothing: it never decides whether an authorization is good, and
it never starts a trial by itself.

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
    [--root <game root>] [--state-dir <dir>]
    (--plan | [--wait <duration>] | --collect <request_id>)
```

The row `standard-pack trial` of the operation table ([lab-operations.json](lab-operations.json),
module `authoring`, slice `PACK-TRIAL`); over MCP the tool `ac_standard_pack_trial`, served by a
`serve` child ([lab-two-entries.md](lab-two-entries.md)). On actinglab's command line the instance
is written `--instance <x>`, as on every own command that acts on one.

**Three forms** (S3-11 lifted S3-10's refusal of a line without `--plan`): with `--plan` the line
answers the plan alone ("The plan"); without it the line is the run ("The run"); with `--collect
<request_id>` it reads the end of an earlier trial's run and records it, sending nothing ("Reading
the end later"). `--collect` or `--wait` beside `--plan`, and `--wait` beside `--collect`, are
`validation_failed` naming that flag with `conflicting`.

**A plan prepares and shows** (the authorization line's first kind, "prepare and show"; the
coordinator's ruling of fix round 2): it needs no authorization and refuses nothing that only the
run needs. What the run will require that the plan was not given, the plan says
(`run_requires`); the refusals that belong to the run (an authorization missing, an irreversible
package not acknowledged) are the run's ("The run").

| Input | What it is |
|---|---|
| `<instance>` | The instance the trial is for. The plan shows it as given; a plan does not contact, resolve or read it. The run is sent to it, resolved as `run start` resolves it; `--collect` reads only a run on it. |
| `--package` | The package directory tried ("The package"). |
| `--authorization` | The reference of the owner's authorization ("The authorization"); optional for a plan, required by the run. |
| `--variable` | A value of a variable the package declares, `<name>=<json>`, once per name ("The variables"); the plan shows it, and the run sends it in its request (S3-8, S3-9, 0.12.68: a Runtime of `runtime-client` revision 11 is needed, "The run"). |
| `--expect` | The outcome key the trial is expected to end with: one the package declares. The run answers whether its end matched it ("The end"). |
| `--irreversible` | Acknowledges a package the irreversible mark flags ("The irreversible mark"); shown in the plan, required by the run of a marked package. |
| `--root` | A game root, installed or a bundle laid out as one, its directory named as its game: the root the irreversible mark is judged over. A package it does not register counts as marked. |
| `--state-dir` | The Lab's state directory, as the recording commands take it (default actinglab's session directory), where trial records are kept ("The record"). |
| `--plan` | Answer the plan only: nothing is sent or written and no Runtime is reached. |
| `--wait` | (S3-11) The longest the run is waited for, a duration as `run start`'s `--wait` takes it; absent, the run is waited for to its receipt ("The run"). |
| `--collect` | (S3-11) The `request_id` of an earlier trial's run whose end was not read: read and recorded, nothing sent ("Reading the end later"). |

**What the plan reads**: the package directory, once, and with `--root` once more for the
irreversible mark ("The irreversible mark"); the game root `--root` names; and whether the entries
of "The state directory's place" lie along the state directory's path. No Runtime, no device, no
installation; it writes nothing (the state directory is not made) and sends nothing.
Under the global `--dry-run` it answers the same ([actinglab-dry-run.md](actinglab-dry-run.md),
`no_effect`); the run and `--collect`, which send or write, are refused there
(`dry_run_unsupported`, after the flags and before anything is read; the alternative is
`standard-pack trial --plan`).

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
`count_out_of_range`, `key_repeated`, or `keys_left_out` for a value taken in part). The plan
judges a trial's value by the package's declaration alone. The run's Runtime judges it again, as
it judges every direct run's value (S3-8, 0.12.68; [package-variables.md](package-variables.md),
"A direct run given values"): by the same declaration, and by whether the variable applies on the
instance now, as the variable table a dispatch reads says it; a value it does not take is its
refusal (`run_variable_undeclared`, `run_variable_not_applying`, `run_variable_value_refused`),
answered as every refusal of the run is ("The run"), and nothing runs.

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

**Without `--root` the mark cannot be judged**: the plan answers `irreversible` `not_judged`. S3-11
decided that the run does not require `--root`: a mark not judged refuses nothing, as the plan
says; only a package the mark flags (`marked`) needs `--irreversible`.

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
beyond it, a flag given twice, S3-11: `--collect` or `--wait` beside `--plan` and `--wait` beside
`--collect` (`conflicting`), `--instance` and `--package` absent, an option without its value,
S3-11: a `--collect` that is no request id (`not_recognised`) and a `--wait` that is no duration
(`wrong_type`), `--variable`; then, without `--plan`, the global `--dry-run`), then the
authorization's bounds, the package path, the state directory's
place, the package's reading and admission, the variables' values, `--expect`, and last `--root`'s
own: a root that is no directory, or whose directory name cannot be read as text
(`validation_failed`); a root file that cannot be read, or is larger than the package input limit
(`validation_failed`); a root without `bundle.json`, or a root file that breaks its shape, a limit
or a reference, a directory name that is not the bundle's game among them (`game_mismatch`)
(`standard_pack_root_invalid`).

## The record

`actingcommand.pack-trial.v1`, one JSON document per trial, at
`<state>/trials/<package_id>/<content12>-<ms>.json`, `<content12>` the plan's
`content_hash_prefix` and `<ms>` the time the trial ended: S3-11 takes the time the Runtime's
ledger recorded the run's terminal, so one run with one content is named the same however often its
end is read; fix round 2: a run that has a record, whatever content it names, is not recorded again
("The end"). It is Lab authoring state beside the recordings: never in a repository, never in an
installed root.

| Member | What it is |
|---|---|
| `schema_version` | `actingcommand.pack-trial.v1`. |
| `at_unix_ms` | When the trial ended (the ledger's time of the run's terminal); it names the file. |
| `plan` | The plan the trial ran by, as `--plan` answers it, but its `authorization` is the reference the run was sent under, never `null` (fix round 3): the record's plan is a type of its own whose `authorization` is a text, made from a plan only by a conversion that refuses a plan without one (`validation_failed`, as the writer refuses another malformed record) and, S3-11, one outside the bounds of "The authorization", by calling that same check (`validation_failed` naming `--authorization`: `value_missing`, `out_of_range`, `not_recognised`), and the reader decodes into it, so a stored record whose `authorization` is absent or `null` is `trial_record_unreadable` `undecodable`. |
| `request_id`, `run_id` | The run's request id, and its run id when it has one. |
| `end` | How the run ended, as its terminal says it: `outcome` (`success`, `failure`, `cancelled`), `outcome_key` (the business end), `final_page`, `failure_code`, `failure_values`, each when it has one. |
| `matched` | Whether `end.outcome_key` is `plan.expect`; absent without `--expect`. |

**A trial that passed** (Workflow #463, the train of S3-12 and S3-14, Runtime 0.12.68;
[v012-register.md](v012-register.md) §1.445 and §1.446, "Train"; the coordinator's ruling, not the
owner's words). A trial passed when its run ended on an end the package declares (`end.outcome`
`success`, the run's own success) and, where the trial was given an expectation (`--expect`), on
that end (`matched` is not `false`). A run that ended normally on another end than the expected one
did not pass: the run itself succeeds, with `trial_end_unexpected` ("The end"), since the business
ended another way, which is what a trial is there to find, but the trial did not pass. Without an
expectation, ending on any declared end passes. One function judges it, the record's
`PackTrialRecord::passed` (beside `unexpected_end`, which warns of the case it does not pass); every
answer that says whether a trial passed calls it.

**What the records say of one content** (the train). One function, `trial_summary`, builds from
every record of a package (read by `trial_records`) the summary of one content that adoption's
review (`review.trial`, [standard-pack-authoring.md](standard-pack-authoring.md), "The trial and
the two warnings") and the status row (`trial`, [standard-pack-schedule.md](standard-pack-schedule.md),
"Where a pack stands") both answer, member for member:

| Member | What it is |
|---|---|
| `state` | `passed` or `failed`, the latest record of this content judged as above; `none` when no record names this content (the vocabulary `trial_state`). |
| `content_hash_prefix` | The content: its 12 hex digits, named by the one function `content_hash_prefix_of`, which the plan, adoption and the status row call. |
| `record` | The latest record of this content: `authorization`, `at_unix_ms`, `request_id`, `end` as the record keeps it, and `matched` when the trial was given an expectation; absent with `none`. |
| `passed_on_record` | Whether any record of this content passed, so that a content that passed once and failed since reads `failed` and `true`. |
| `other_content` | A record of another content of the same package is on record; it counts for nothing. |

When the reader refuses the package's records (a file of a record's name that is no record, or the
directory not listed), the answer carries instead `trial_not_read`, `{outcome, text}`: the reader's
refusal with its code, and the same in words. Neither answer is refused for it, and every other
part of it is answered as before.

**The state directory's place.** A state directory that is, or lies inside, an installed root or a
repository is refused `trial_state_dir_refused` with its `path` and the word `trial_state_place`:
`installed_root` when a directory along its path (made absolute, and again with its nearest
existing part's links resolved) has `install\state.json` as a file, the installer's own state
file in both layouts ([install-cli.md](install-cli.md), "Layout"); `repository` when one holds
`.git`. The installed root is recognised by one function of the resource tooling,
`installed_root_containing`, which the plan and the writer call. It sits below the Lab's crate,
which may call it, but the Lab's recordings keep their own walk (`installed_root` of
`crates/lab/src/recording/container.rs`, #464 P4: an output directory inside an installed root is
`record_lab_dir_invalid` `inside_installed_root`): the same file along the same two paths, but a
check the operating system refuses is read there as "absent" and the walk goes on, and a path that
cannot be made absolute is walked as given, where this function answers `foreign_os_error`
(`validation_failed` for that path). The two name the same root only while every check is
answered, so the recordings were not pointed at this function. The Runtime host's
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
Lab answer carries warnings in. S3-11: the run calls the writer once for each run whose end it
read ("The end"), and only then.

**The writer's steps** (S3-11, the review of S3-10's third round): the hard link of the `.part` and
the write and flush of a file created anew are taken as parameters inside the module, as the
Runtime host's `place_once` takes its link (`state_root_installation.rs`), so that the tests drive
each path the operating system's refusals take: the exclusive create where no hard link can be
made, the success with `trial_record_part_left`, the removal of this call's files after a refused
write or flush (`io_op` `sync` for the flush), and a removal refused in its turn.

**The reader** answers every record of a package (`trial_records`), in the order of their times
(then by content), and the latest record of one content of a package (`latest_trial_record`); none
when there is none. A file of a record's name that is not read as the record its name says is
`trial_record_unreadable` (`path`) with the required word `trial_record_problem`: `read_failed`
(the operating system refused to open, read or list it; its refusal as the cause), `too_large`
(larger than 1 MiB), `undecodable` (no such document, a plan without an authorization among them;
the decoder's refusal as the cause) or `name_mismatch` (a record, but not of the package, the
content and the time its name says): the reader answers no record of that package rather than pass
over one. A file of another name, a `.part` among them, is no record. The status read (S3-14) and
adopt's review (S3-12) call it, both through `trial_summary` (above). S3-11 fix round 2: `trial_record_of_run` answers, through
it, the file of the record of one run's request id, whatever content it names; the run calls it
before the writer.

## The run

Slice S3-11. A line without `--plan` and without `--collect` is the run. Before anything is sent
and before anything is written:

1. **Everything `--plan` does**, by the same function (one reading of the package), with its
   refusals in their order ("The plan").
2. **The refusals only the run needs**, in this order (the resource tooling's `check_trial_run`),
   each with its one code and one cause:
   - no `--authorization`: `trial_authorization_missing` (`package_id`);
   - a package the irreversible mark flags (`marked`) and no `--irreversible`:
     `trial_irreversible_unacknowledged` (`package_id`); a mark not judged (no `--root`) refuses
     nothing.

   S3-8, S3-9 (0.12.68): a `--variable` refuses nothing here any more; the run carries the values
   the plan shows as given, and `trial_variables_not_carried`, which refused it while a direct run
   could carry none, is retired (raised nowhere; its catalog entry stays, `retired`).

   The state directory refused is S3-10's `trial_state_dir_refused`, already the plan's. A record
   of this content at the run's millisecond already there is S3-10's `trial_record_exists`, which
   only the writer can find: the record's name carries the time the run ended. So is a record of
   this run, whatever content it names (fix round 2), found just before the writer ("The end").
3. **The Runtime is reached** as every own command of actinglab reaches it (a Runtime that does not
   answer is `runtime_not_running`), on the Lab channel with the initiator the process has.

**One run, sent once.** The request is the one `run start` sends, made by its own function
(`crates/lab-ops`, `core_ops::run_start_prepared`, called, not copied): the instance resolved by
one status read, the package directory tried by its path made canonical, the longest deadline a
run may have (`ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS`, 30 minutes), no recovery package,
and (S3-8, S3-9, 0.12.68) the values `--variable` gives, handed to `run start`'s own `variable`
parameter, each `<name>=<the value as compact JSON>`, so that `run start`'s parser reads them into
the request's `variables`; without `--variable` the request carries none and is the request of
before. A run with values needs a Runtime of `runtime-client` revision 11: one below it cannot
decode the request and ends the connection, nothing runs, and that is the client's failure,
answered as every lost connection below. It is named by its `request_id` before it is sent, and
submitted once. It is waited for as `run start`
waits: without `--wait` to its receipt, the run's deadline and the client's margin bounding the
wait (the longest wait `run start` allows, `run start` without `--wait`); with `--wait` up to that
long (`wait::wait_at_most`), the run going on in the Runtime when the wait ends first. The client's
own handling of a run is `run start`'s (its reset after a cancelled run, its recovery of a receipt
that did not come in time); nothing else is sent, and this command never sends the run again:

- a wait that ends first is `command_wait_expired` (`operation` `run_contained_task`,
  `request_id`, `waited_ms`), as `run start` answers it: no record is written, and the answer says
  so with `request_id` and `next`, the line that reads the end later ("Reading the end later");
- a Runtime that refused the run (a receipt came back, and no terminal) answers its refusal, as
  `run start` does, beside `request_id`: no run ran and nothing is written;
- otherwise the run's end is read ("The end"): after a receipt with a terminal or a completed one,
  and after a result that is not known (no receipt came back). That read goes on the same
  connection: a connection lost after the request left answers it at once with the client's
  failure and sends nothing, and that failure is the answer, beside `request_id` and `next`;
  nothing is written and nothing is sent again. (A run the client saw cancelled comes back without
  its receipt, after the client's own reset; its end is read and recorded as a cancelled run's.)

## The end

The run is read as `run show` reads it (`lab-ops` `run_show::read`, its key the request id): its
state and its terminal, whose `outcome_key` is the key of the mapping of the page the run finished
on (since S3-3, [linear-steps.md](linear-steps.md), "outcome key"). With a terminal, the ledger's
record of that terminal is read once more by its request and its sequence, for the time it was
recorded, which names the record. The answer:

| Member | What it is |
|---|---|
| `sent` | `true` for the run; `false` for `--collect`. |
| `plan` | The plan, as `--plan` answers it. |
| `request_id`, `run_id` | The run's request id, and its run id when the read names one. |
| `end` | `{state, outcome_key?, final_page?, failure_code?, failure_values?}`: `state` the run's state as `run show` names it, the rest its terminal's, each when it has one; `outcome_key` exactly when `run show` has one. |
| `expected`, `matched` | With `--expect`: the key given, and whether the end's key is that key (`false` when the run failed, was cancelled, or ended with another key or none). |
| `record` | The record's file, written once ("The record"), and `part_left` when the writer left its `.part`. |
| `warnings` | `trial_end_unexpected` when the run ended normally on another end than the one expected, and the writer's `trial_record_part_left`. |

- **The end state decides** (fix round 2): whether the run failed is read from `end.state` as `run
  show` names it, never from whether its terminal names a code.
- **A run that ended normally** (`succeeded`) succeeds, exit 0. **On another ending than the one
  expected** it still succeeds, `matched` `false` with the warning `trial_end_unexpected`
  (`package_id`, the end's `outcome_key` when it has one): the business ended another way, which
  is what the trial is for.
- **A failed or cancelled run** is answered with its code and values on top (the terminal's
  `failure_code` and `failure_values`) and no key, beside the same members, the record written: the
  command fails as `run start` answers a failed run (exit 1 on actinglab's command line; the
  envelope of a `--via` line by its class, 4). One whose terminal names no failure code (a ledger
  may hold such a terminal) is `trial_end_uncoded` (`request_id`, `run_state`) on top instead, the
  record written with its end as read, and fails the same way: never a success.
- **The record** is written once, by S3-10's writer, when the end has been read: the plan with the
  authorization, the request id, the run id, the end (`outcome` the terminal's, `outcome_key`,
  `final_page`, `failure_code`, `failure_values`), the expected key (the plan's `expect`) and
  `matched`. **One run, one record** (fix round 2): just before the writer, the package's records
  are read (`trial_record_of_run`, by the reader `trial_records`); a record of this run's request
  id there, whatever content it names, is `trial_record_exists` with `path` that record, and
  nothing is written; the answer carries no `next`, since the run is recorded. A refusal of that
  reading (`trial_record_unreadable`) or of the writer (`trial_record_exists` for a name taken,
  `trial_record_write_failed`, `trial_state_dir_refused`) is answered with its code beside the same
  members and `next` (fix round 2): the run happened and only its record is missing.
- **An end not read writes no record** and says so, each cause with its own code (fix round 2):
  - a read the client refuses is answered as that with `request_id` and `next`;
  - a run that has not ended yet (`not_found`, `admitted`, `running`) is `trial_end_unread`
    (`request_id`, `run_state`) with `next`: read it again later;
  - a run interrupted by a later Runtime start or takeover without a terminal
    (`interrupted_unterminated`) is `trial_run_interrupted` (`request_id`) without `next`: it has no
    end to record, and this command sends nothing that would give it one;
  - a run whose read names a terminal (`succeeded`, `failed`, `cancelled`) whose record the second
    read, by the request at the terminal's sequence, did not find is `trial_terminal_unrecorded`
    (`request_id`, `run_state`) without `next`: look at the run's facts in the ledger first.

  Each answers `end: {state}` beside the members.

`next` is `{collect: [...]}`: the words of actinglab's command line after the program's name, the
trial's own arguments (`--instance`, `--package`, `--authorization`, since S3-8, S3-9 each
`--variable` as the run was sent with it, `--expect`, `--irreversible`, `--root`, `--state-dir` as
the trial had them) with `--collect <request_id>`.

## Reading the end later

`--collect <request_id>` beside the same arguments: the plan and the run's own refusals as for the
run; then it **sends nothing**: one status read resolves the trial's instance, and the run is read
as `run show` reads it. A run with no fact at all is `trial_end_unread` `not_found`. Any other run
is refused `trial_collect_mismatch` (`request_id` and the word `trial_collect_problem`), in this
order, when its facts:

- do not name the trial's package as the package it admitted: `other_package`;
- do not name the instance the trial is for: `other_instance`;
- (fix round 2) name the scheduler as its sender (`dispatch` `scheduled`): `scheduled`;
- (fix round 2) hold no record of its request's origin (`dispatch` `unknown`), so that whether it
  was sent by hand is not known: `origin_unknown`;
- (fix round 2) name a manual run sent on another channel than the Lab's (`origin` `cli` or `ui`):
  `other_channel`. A trial's run is sent on the Lab's channel.

Then its end is answered and recorded as the run's is ("The end"), `sent` `false`. It is how a
trial that outlasted its wait gets its record.

**What `--collect` can know and what it cannot** (fix round 2). It knows what the run's facts say:
its package's `package_id`, its instance, that it was sent by hand (`dispatch` `manual`) and on the
Lab's channel (`origin` `lab`), and its end. It cannot tell a trial's run from an ordinary `run
start` of the same package on the same instance sent through actinglab or the Lab's MCP server:
the request is the one `run start` sends, by the same function, with the same initiator, and
nothing recorded with the run (its request's origin record names the channel, never the command)
says which command sent it. Such a run, given to `--collect`, is recorded as a trial. Nor can it
know the content that ran: the run's facts name the package by its `package_id` and record no
content digest (option 丙 of #336), and the trial keeps nothing before its record. The content a
`--collect` records is the plan's, the package directory as read at the time of the `--collect`;
when that directory changed after the run, the record names content that was not what ran. Nor
can it know the values the run was given (S3-8, S3-9): a direct run records nothing of them, so
the values a `--collect` records are its own line's `--variable`s, the ones `next` repeats. What
holds: a run has one record. Before the writer, a record of the run's request id is looked for
among the package's records whatever content it names, so a second `--collect` of a run that has
its record, after the directory changed or not, is `trial_record_exists` naming that record and
writes nothing ("The end"). The line is the trial's own, which `next` repeats. Under `--dry-run` it
is refused, as the run is.

## Never by itself

No trial follows an adoption, a build or a placement; nothing retries a trial, runs it in a loop,
changes a hold or schedules one. A passed trial is not an authorization to enable; a trial counts
nothing toward a break-in; a trial is one run, sent once.

## Outcome codes

Each refuses (category `error`, class `none`) but `trial_record_part_left` and
`trial_end_unexpected`: exit 1 on actinglab's command line, which prints the CLI module's line,
and in the envelope a `--via` line prints exit 2, but 4 for `trial_end_unread`,
`trial_run_interrupted`, `trial_terminal_unrecorded` and `trial_end_uncoded` (fix round 2), whose
class is the run's (`device_instance`), as the run's other answers that are no refusal of the line
(a failed run, a relayed refusal, `command_wait_expired`). `trial_record_part_left` warns (category `warning`, class `none`, fix round 3): it rides in
the `warnings` of a successful write and leaves its exit at 0. The command line's own refusals and
the malformed arguments are `validation_failed`, as above; a check of the state directory's path
the operating system refuses is `foreign_os_error`.

| Code | The one situation | Refuses / warns |
|---|---|---|
| `trial_state_dir_refused` | The state directory is, or lies inside, an installed root or a repository (`trial_state_place`). | refuses |
| `trial_record_exists` | A record or its `.part` of that name is already there: it is never replaced; a person looks at that file. Fix round 2: or the run being recorded has its record already, whatever content it names (no `next`). | refuses |
| `trial_record_write_failed` | The operating system refused a step of the write, and no record was written. | refuses |
| `trial_record_unreadable` | A file of a record's name is not that record (`trial_record_problem`). | refuses |
| `trial_record_part_left` | The record is written and in place, but the `.part` its write created could not be removed. | warns |
| `trial_authorization_missing` | (S3-11) The run was asked for without `--authorization`. | refuses |
| `trial_irreversible_unacknowledged` | (S3-11) The mark flags the package and the run was asked for without `--irreversible`. | refuses |
| `trial_variables_not_carried` | (S3-11; retired in 0.12.68 by S3-8, S3-9, raised nowhere) The run was asked for with a `--variable`, which a direct run could not carry before 0.12.68. | refused (retired) |
| `trial_end_unexpected` | (S3-11) The run ended normally on another end than `--expect` named; the command succeeds. | warns |
| `trial_end_unread` | (S3-11) The run has not ended yet (`run_state` `not_found`, `admitted`, `running`); no record was written; `next` reads it again later. | refuses |
| `trial_collect_mismatch` | (S3-11) `--collect` names a run that is not a trial's run of this package on this instance (`trial_collect_problem`: `other_package`, `other_instance`; fix round 2: `scheduled`, `origin_unknown`, `other_channel`). | refuses |
| `trial_run_interrupted` | (S3-11 fix round 2) The run was interrupted (`interrupted_unterminated`): it has no end to record; no record, no `next`. | refuses |
| `trial_terminal_unrecorded` | (S3-11 fix round 2) The run's read names a terminal (`run_state`) whose ledger record was not found at its sequence; no record, no `next`. | refuses |
| `trial_end_uncoded` | (S3-11 fix round 2) The run ended failed or cancelled (`run_state`) and its terminal names no failure code; the record is written, the command fails. | refuses |
