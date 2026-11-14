# State root installation record

Workflow #453 S11 (Runtime 0.12.60; register [v012-register.md](v012-register.md) §1.358). The
owner approved board #453; the coordinator's rulings on the board replace the design survey's
identity by the configuration's salt. Before this slice nothing in a state root said which
installation it belongs to, so a configuration that names another installation's state root
started on it whenever that installation was not running. Since this slice a Runtime started from
an installation root records, in the state root, which installation claimed it, and refuses to
start on another installation's state root. Workflow #453 S12 (Runtime 0.12.60; register §1.364)
brings the command that hands a state root over to an installation, `actingd claim-state-root`
("Handing a state root over"), and its rescue row `state-root claim`.

## The installation's identity

`<installation root>\install\identity.json`, schema `actingcommand.installation-identity.v1`:

```json
{"schema_version":"actingcommand.installation-identity.v1","installation_id":"<32 lowercase hex>","minted_at_unix_ms":1800000000000,"minted_by":{"pid":4242,"build_commit":null}}
```

- `installation_id`: 16 bytes from the operating system's random source
  (`actingcommand_host_metrics::fill_system_random`, `BCryptGenRandom` with the system-preferred
  generator), written as 32 lowercase hexadecimal digits. It is not a secret and it is in no
  configuration: copying another installation's configuration (and with it its
  `secret_fingerprint_salt`) copies no identity, which is exactly the case the record catches.
- `minted_at_unix_ms`, `minted_by.pid`: the start that minted it; `minted_by.build_commit`: the
  commit its build was made from, `null` for a build that does not know it.
- Minted once, by the first start of the installation that finds none (see "The claim"), and
  **placed only when absent** ("Placing a file once" below): two configurations of one
  installation root that start for the first time at the same moment each mint a number, and the
  file the first of them places is the installation's; the other start keeps it and records its
  number. Besides a start only `actingd claim-state-root` writes it: it mints one exactly as a
  start does when there is none, and with `--new-identity` it sets one that cannot be read aside
  as `identity.json.old-<ms>` and mints a new one ("Handing a state root over"); it never
  replaces one that can be read. Nothing else writes, moves or renames it; the installer leaves it
  as it is ("The installer" below).
- What one installation is: whatever carries the same file. A moved installation folder, a
  restored backup of it and two builds of one installation carry the same file and are one
  installation. A copy of a whole installation root carries it too and is the same installation:
  that limit is accepted and has no rule.

## Which root is the installation root

The **installation root** of a program that writes the state root (a start, `actingd
unlock-owner`, every `actingd ledger-maintenance` pass, and since S12 `actingd claim-state-root`,
which without one refuses) is the directory of its
`--config` file **when that directory holds the installer's own `install\state.json`**
([install-cli.md](install-cli.md), "Layout"). No program is given a new flag: the installer's
start and the watchdog start `<root>\runtime\actingcommand-actingd.exe --config
<root>\actingd.config.json` ([runtime-watchdog.md](runtime-watchdog.md), "Start"), whose
directory is the root. `check-config` takes the root its `--install-root` names, else the
directory of `--config`, each only when it holds `install\state.json`.

A program not started from an installation root (CI's start from the example configuration, a
developer's run, a configuration in any folder without the installer's state file) **mints
nothing, records nothing and checks nothing**: it reads neither file and writes not one byte more
than a build before this slice. What it does more is one read of file metadata, the look for
`install\state.json` beside its configuration. A failure of that read other than "not found"
refuses the start, `actingd unlock-owner`, every `actingd ledger-maintenance` pass, `actingd
claim-state-root` and `actingd check-config` with `state_root_installation_record_invalid` (`path` that
`install\state.json`, the operating system's failure as its `caused_by` link): whether the
program belongs to an installation is then unknown, and the check is not skipped silently.

**Not protected by this slice:** an A/B root of the older installer whose configuration is given
from `install\generations\…` has no `install\state.json` beside that configuration, so a program
started with it is `not_installed` and checks nothing. The installer's contract says that no A/B
root is in use ([install-cli.md](install-cli.md), the paragraph on `tools\actingwatch.exe`).

## The state root's record

`<state root>\installation.json`, schema `actingcommand.state-root-installation.v1`:

```json
{"schema_version":"actingcommand.state-root-installation.v1","installation_id":"<32 lowercase hex>","claimed_at_unix_ms":1800000000000,"claimed_by":{"pid":4242,"owner_epoch":"epoch_<32 hex>","build_commit":null}}
```

- `installation_id`: the claiming installation's number; `claimed_at_unix_ms`, `claimed_by.pid`,
  `claimed_by.owner_epoch` (the claiming start's, as its owner journal record spells it) and
  `claimed_by.build_commit` (`null` for a build that does not know it): the start that claimed it.
- Written once, **placed only when absent** as the identity is ("Placing a file once" below);
  it is read absent under the owner lock just before. No Runtime writes it again, renames it or
  deletes it. Only `actingd claim-state-root` replaces it: it sets the record aside as
  `installation.json.old-<ms>` (kept, never deleted) and places this installation's ("Handing a
  state root over").
- **A registered exception to "program state lives in tagged ledger records"** (iron rule 13), as
  the integrity key `runtime-state.key` ([runtime-database.md](runtime-database.md)) and the
  watchdog's `<root>\watchdog\state.json` ([runtime-watchdog.md](runtime-watchdog.md), "Files")
  are: the check must read it before the ledger is opened, and before anything is written.
- **Mirrored once into the ledger** as the tagged runtime fact `host.state_root_installation`
  ([runtime-fact-store.md](runtime-fact-store.md), "Producers"), for the audit trail: runtime
  scope, `record_list` of one row `{claimed_at_unix_ms, claimed_by_pid, claimed_by_owner_epoch,
  claimed_by_build_commit?}` taken from the record (`claimed_by_build_commit` absent when the
  record's `build_commit` is `null`), observed at the record's claim time, source `runtime`, no
  lifetime. The installation's number is not in it. It joins the default of
  `ledger_observer.tag.workbench_patterns` ([ledger-observer.md](ledger-observer.md) §15), so a
  start loads it. **The fact follows the record**: the start that placed the record records the
  fact once the ledger is open; a start that finds this installation's record while the fact
  store holds no `host.state_root_installation` (an earlier start placed the record and failed
  before the fact, for example at a ledger migration requirement) records the fact from the
  record, once. **Since S12 a start compares the claims**: it records the fact from the record
  whenever the latest `host.state_root_installation` the fact store holds is not of the record's
  claim (another `claimed_at_unix_ms` or another `claimed_by_owner_epoch`), and nothing when it
  is. So after a hand-over by `actingd claim-state-root`, which never opens the ledger, the
  ledger still holds the old claim's fact, and the next start records the new claim once; a
  restart finds its own claim and records nothing. Such a row also carries the optional member
  `previous_claimed_at_unix_ms`, the stored claim's time, and the record is observed at the
  claim's time or one millisecond after the stored fact's observation when the claim's time is
  not later, so that the store takes it as the latest. The rules before S12 are the two cases of
  this one rule (no fact stored; the fact of an earlier claim stored), so no start records
  anything it did not record before but the new claim after a hand-over (and the claim of a start
  that placed a record after a hand-over by hand, which then also carries the member).

## The check

A start from an installation root reads the identity file and the record **beside the format
refusal, before anything is written**: before the state root's folder is created when it is
missing (a missing state root holds no record) and so before the owner journal:

| Found | Answer |
|---|---|
| no record | the start goes on |
| a record with this installation's number | the start goes on |
| a record with another number | refused: fatal `state_root_installation_mismatch` |
| a record while this installation has no identity file | refused: fatal `state_root_installation_mismatch` |
| a record or an identity file that cannot be read, is larger than 64 KiB, is not the document or has another `schema_version` or an `installation_id` that is not 32 lowercase hexadecimal digits | refused: fatal `state_root_installation_record_invalid` |

`state_root_installation_mismatch` names the state root as `path` and the record's claim time as
`at_unix_ms`; its text (the daemon's developer line, and the native text the outcome carries)
says which of its two cases it was. `state_root_installation_record_invalid` names the file as
`path` with the read failure as its `caused_by` link (`foreign_os_error`, or `foreign_data_error`
with `raw_source` `json` for a document that is not what it must be). Either refusal writes
nothing: the owner journal is untouched, nothing is minted, and the watchdog holds the fatal
refusal down ([runtime-watchdog.md](runtime-watchdog.md), row 5′).

## The claim

A start claims, **under the owner lock**, right after it took it
(`OwnerGuard::acquire`, which wrote the start's owner record). Besides a start, the one program
that mints an identity and claims a state root is `actingd claim-state-root`, as the explicit act
of a person ("Handing a state root over" below); every other program only checks. A start's
claim:

1. The record is read again. A record of this installation: nothing is written. A record of
   another installation, or a record while this installation has no identity file (a record that
   appeared since the check: two starts raced): the same refusal as the check's, after the
   acquire, and the start closes the owner epoch it opened as any failed start does.
2. No record: when the installation has no identity file, a number is minted and the identity
   file placed; the identity is then **read back from the disk** and its number is the one used
   (a file another configuration of the installation placed first is kept and its number taken).
   The record is placed with that number and this start's owner epoch, and read back and judged
   as the check judges it.
3. Once the ledger is open, the fact `host.state_root_installation` is recorded from the record
   ("The state root's record": the fact follows the record), and actingd writes one daemon line
   `state_root_claimed` (info, `path` the state root) for the start that placed the record only;
   a later start that records a missing fact from the record writes no line.

A step that cannot be done (the random source fails; a `.part` cannot be removed, created,
written or flushed; the placement fails, by the hard link and then by its fallback ("Without
hard links" below), other than because the file is there; the part's name
cannot be removed afterwards; a placed or found file is gone when it is read back) is fatal
`state_root_claim_failed` (`path` the file, the operating system's failure as its `caused_by`
link); the start fails and closes its epoch, and the next start claims again.

So an existing installation is recorded by its first start of this release, with that one line,
and a restart reads its own record and writes nothing more.

### Placing a file once

The identity file and the record are each placed with a primitive that fails when the target
exists: the bytes are written to `<file>.part`, flushed to disk, and the part is then
**hard-linked** to the target (`std::fs::hard_link`: `CreateHardLinkW` on Windows, `link`
elsewhere), after which the part's name is removed. Creating a hard link adds one directory
entry for an existing, complete and flushed file in a single file-system operation, and it fails
with "already exists" when the target name is taken: it never replaces a file, unlike a rename,
which the operating system lets replace its target. So on Windows (NTFS) the target appears
complete, or not at all, and a file another writer placed first is kept as it is; the claim then
reads back what the disk holds. A rename is not used.

**Without hard links.** A hard link that fails for any other reason than "already exists" (a
file system without hard links, such as FAT32 or exFAT on a removable drive) falls back to an
**exclusive create of the target itself** (`create_new`): it fails when the target exists, and
otherwise the bytes are written and flushed. "Already exists" on either the link or the create
means another start placed the file: it is kept, read back and used. Either way the claim then
reads the file back from the disk and uses that value, exactly as after a link. Only when the
fallback fails too is the claim failed, `state_root_claim_failed` (the fallback's failure as its
`caused_by` link, the link's as a `note`). What the fallback gives up: the file appears before
its few bytes are written, so on such a file system a crash while they are written can leave an
incomplete file, which the next start refuses as `state_root_installation_record_invalid`; the
way out is the one for a corrupt file below ("When a start is refused").

A crash between these steps can leave `identity.json.part` (or `installation.json.part`), which
the installer's `install leftovers` lists as a `.part` file. The next mint reuses that name: it
removes the stale part first and creates it anew (removing the name, never writing through it,
so a file it may still name as a second link, such as a file a person set aside, keeps its
bytes).

## Handing a state root over: `actingd claim-state-root`

Workflow #453 S12 (register §1.364). The way out of a refusal when this installation is meant to
own the state root ("When a start is refused" below): what a person did by hand before, set the
record aside and let this installation claim the state root, done by one command that keeps
every file it replaces.

```text
actingd claim-state-root --config <path> --actor <name> [--new-identity]
```

Options may appear in any order, each at most once. `--config` names the daemon configuration; of
it only `state_root` is used (loaded and checked as `actingd unlock-owner` loads it, its refusals
at stage `config_load`); the salt is not read for anything. The installation root is the
directory of `--config` when it holds the installer's own `install\state.json` ("Which root is
the installation root"). `--actor` names who asked, with `unlock-owner`'s rules for the name (1 to
256 bytes, path-safe); the rescue row passes its initiator. `--new-identity` is the person's
statement that the installation's identity file, which cannot be read, is to be replaced. A
command line it does not take is `invalid_arguments`.

**Order.** Every refusal of steps 1 to 3 writes nothing.

1. Without an installation root the command refuses `state_root_claim_failed` (`path` the state
   root; the text says the configuration's folder holds no installer state file): only an
   installation claims a state root.
2. The identity file is read: one that cannot be read refuses
   `state_root_installation_record_invalid` (`path` the file, the read failure as `caused_by`), as
   a start refuses it, unless `--new-identity` is given; `--new-identity` while the file can be
   read refuses `invalid_arguments`: the command never replaces an identity that can be read.
3. The state root's folder is made when missing, as a start makes it. The owner is taken as a
   start takes it (`OwnerGuard::acquire`): a Runtime that holds the owner lock refuses the command
   with the existing `owner_conflict`, and a previous owner whose resources are recorded in use
   and whose process still runs refuses it with `owner_resource_unconfirmed`, as it refuses a
   start. The acquire writes the command's own owner epoch, its pid and time, to the owner
   journal: **the owner journal records the act**, and the new record names that epoch as
   `claimed_by.owner_epoch`. **What taking the owner as a start and the maintenance passes take it
   costs** (fix round 1, the coordinator's ruling): an earlier epoch that ended in a crash (its last
   record still active) is taken over by this command and closed by it, so the next start finds an
   inactive last record and gives that epoch's instances no takeover cooldown, as after a ledger
   maintenance pass; and an owner that had exited with its resources recorded in use is released
   as a start releases it but with no ledger record of the release (a start records
   `runtime.lifecycle_observed` and writes the daemon line `owner_released`; this command opens no
   ledger and writes no such line).
4. Under the lock, **the identity**: one that can be read is used as it is (`read`); none (an
   installation that never started) is minted and placed only when absent, exactly as a start
   places it, and read back (`minted`); one that cannot be read, with `--new-identity`, is set
   aside as `install\identity.json.old-<ms>` and a new one minted and placed (`renewed`). A
   readable one with `--new-identity` is refused here too.
5. **The record**: none is written, what this installation's first start would have done
   (`written`); this installation's own changes nothing (`unchanged`, and the answer says nothing
   changed); another installation's, or one that cannot be read, is set aside as
   `installation.json.old-<ms>` and this installation's written (`replaced`). The new record is
   `{schema_version, installation_id, claimed_at_unix_ms, claimed_by {pid, owner_epoch,
   build_commit}}` with the command's time, pid, owner epoch and build, read back and judged as
   the check judges it.
6. The command's owner epoch is closed, also after a failure of steps 4 and 5.

**Placing and setting aside.** Both files are placed with the start's primitive ("Placing a file
once": a hard link of a flushed `.part`, an exclusive create of the file itself without hard
links). A file is set aside by giving it its kept name `<file>.old-<ms>` with a hard link, which
fails when the name is taken, so no kept file is ever replaced, and then removing its old name;
without hard links it is renamed, only while the kept name is free. **Nothing is deleted on any
path**: every replaced file keeps its bytes under its kept name. When the placement after a
set-aside fails, the set-aside file is given its name back (a hard link, or a copy by an exclusive
create), so the failure leaves the file where it was; only when that fails too is the state root
left without the file, the failure's note saying so. A crash between the steps is completed by
running the command again: a state root left without a record gets this installation's
(`written`); a record already this installation's changes nothing. After a crash past the new
identity's placement the identity can be read, so the command is run again without
`--new-identity` (with it, it is refused and writes nothing).

**It never opens the ledger.** The ledger, the integrity key `runtime-state.key` and the owner
journal's earlier records are not touched; the journal gains only the command's epoch. The
ledger's fact of the new claim is the next start's to record ("The state root's record": the
start compares the claims), with the replaced claim's time as `previous_claimed_at_unix_ms`.

**Who did it is not recorded** (fix round 1, the coordinator's ruling). The actor (`--actor`) is
in the command's report and the rescue row's answer only: it stays out of the owner journal, whose
record has no member for it, and out of the fact, and the record's shape is not changed. What a
later reader can still find: the new record's `claimed_by.owner_epoch`, `pid` and
`claimed_at_unix_ms`, which name the owner journal's epoch of the same id, with that epoch's pid,
its start and its close; the old record, kept as `installation.json.old-<ms>` (the old claim, and
in its name the time it was set aside); and the next start's fact of the new claim with the
previous claim's time, `previous_claimed_at_unix_ms`.

**Result.** One JSON line on stdout, schema `actingcommand.actingd.claim-state-root.v1`, exit by
the outcome's category (0 for the success, 1 for an error, 2 for a fatal refusal):

```json
{"schema_version":"actingcommand.actingd.claim-state-root.v1","outcome":{"schema_version":"actingcommand.outcome.v1","code":"state_root_claimed","category":"info","values":{"path":"C:\\ActingCommand\\state"}},"actor":"Lab-CLI","changed":true,"identity":"read","record":"replaced","claimed_at_unix_ms":1800000000000,"kept":["C:\\ActingCommand\\state\\installation.json.old-1800000000000"],"previous":{"claimed_at_unix_ms":1700000000000},"kept_record":"C:\\ActingCommand\\state\\installation.json.old-1800000000000"}
```

The success is `state_root_claimed` (`path` the state root), the code a claiming start's daemon
line carries. `changed` is false only when the record was this installation's and its identity was
read. `previous` is present when a record that could be read was replaced. `kept_record` (fix
round 1) is the kept old record the salt's caveat is about: for `replaced` the record set aside
now; for `written`, the newest `installation.json.old-<ms>` the state root already held, when it
held one (a crash between setting the old record aside and placing the new one, or a put-back that
failed, looked for before anything is placed); absent for `unchanged`. A refusal is the refusal's
outcome alone, its developer line on stderr.

**The salt.** Ledger records that carry fingerprints were made with the salt of the configuration
that wrote them. The command cannot know whether this configuration's salt is the same as the one
the replaced record's installation used, so every answer that replaced a record of another
installation, or one that could not be read, says so (the rescue row's caveat
`fingerprints_discontinuous`), and so does (fix round 1) an answer that wrote a record while the
state root held a kept old record, since the replacement that left it there never answered; a
state root taken over keeps those records as they are.

**The rescue row `state-root claim`** ([install-cli.md](install-cli.md), "The Lab's rescue
module"; [lab-operations.json](lab-operations.json)): `writes`, `irreversible`, `runtime`
`offline`. It runs the live build's `actingd claim-state-root --config <the root's
configuration> --actor <initiator>` (and `--new-identity` when given), as the row `unlock` runs
`unlock-owner`, and only while the Runtime is stopped by the update path's stopped rule
(`owner_lock_held`, `runtime_running`, `processes_unreadable`). Its answer is
`install_command_done` (operation `install_state_root_claim`, `installation_changed` as actingd's
`changed`) with actingd's report, `previous {claimed_at_unix_ms}` when a record was replaced, and
the note `install_caveat_noted` with `install_caveat` `fingerprints_discontinuous` and actingd's
`kept_record` as `path` whenever a record of another installation or one that could not be read
was replaced, and (fix round 1) whenever a record was written beside a kept old record. Like every
compound operation of the rescue module until LAB-7, it is not used on the production root. The
rescue module's `op status` reads the state root's installation state only when asked with its
switch `state_root_installation` ([install-cli.md](install-cli.md)): without it, it starts no
process.

## Who checks and who does not

- **A start** checks and claims (above).
- **`actingd claim-state-root`** (S12) is the check's way out: it does not refuse another
  installation's record or one that cannot be read, it replaces it and keeps it ("Handing a
  state root over"); it refuses an identity file that cannot be read without `--new-identity`.
- **`actingd unlock-owner`** ([actingd-unlock-owner.md](actingd-unlock-owner.md)) and **every
  `actingd ledger-maintenance` pass** (`dry-run`, `import`, `backup`, `restore` and `verify`,
  with or without `--full` and `--frame-query`; [ledger-maintenance.md](ledger-maintenance.md))
  make the same read-only check before they take the owner lock, with the same codes
  (`check_state_root_installation` their operation), and **never mint and never claim**. Every
  maintenance pass takes the owner and so writes to the state root (the owner journal's open and
  close records), `verify` without flags included: a program that writes anything to a state
  root, the owner journal included, checks first. So the installer's ledger verify, which runs
  the target build's `verify` without flags with the root's configuration, refuses another
  installation's state root as well when that build is of this release or later. Their
  installation root is found by the same rule.
- **Readers that take no owner do not check**: the forensic readers (`actingledger`) and the
  watchdog.
- **`actingd check-config`** ([actingd-check-config.md](actingd-check-config.md)) makes the check
  read only and reports `state_root_installation`: `claimed_by_this`, `unclaimed` or
  `not_installed` (no installation root: nothing read). The other two states refuse with the
  start's codes at stage `config_state_root`: `claimed_by_other` is
  `state_root_installation_mismatch`, `record_invalid` is `state_root_installation_record_invalid`
  (a refused check-config prints no report). It never mints and never writes. So every installer
  path that runs check-config before it changes anything (`config replace`, `instance add`, the
  update plan, `start`) refuses a configuration that names another installation's state root.

## The installer

No installer command writes, moves, renames, keeps or lists `install\identity.json`
([install-cli.md](install-cli.md), "Layout"): the update, the rollback, an undo, `status`,
`leftovers` and `place` leave it as it is, and the A/B clean (R-15d) never touches it. The readers
of `install\` that list the folder are `install leftovers` (it lists only names that carry a kept
marker, `.old-`, `.rolledback-`, `.abandoned-`, `.prev-`, `.finished-`, `.part`) and the gate tests'
tree comparisons; none refuses an unknown file and none needed a change. An identity file
`actingd claim-state-root --new-identity` set aside, `install\identity.json.old-<ms>`, carries the
kept marker, so `install leftovers` lists it as the kept file it is; no installer command removes
it. No new `actingctl` command comes with S11 or S12; S12's rescue row `state-root claim` runs
actingd's command, and, as every compound operation of the rescue module until LAB-7, it is not
used on a production root.

## When a start is refused

The Runtime stays down (fatal) and the start's log names the configured state root and the time
the record's installation claimed it.

- **The configuration is wrong** (the usual case: a configuration copied from another
  installation root, or a `state_root` edited by hand): correct `state_root` so that it names this
  installation's own state root, or put the previous configuration file back. Nothing was written
  to either installation.
- **This installation is meant to own that state root**: hand it over with `actingd
  claim-state-root --config <this installation's configuration> --actor <name>` (the rescue row
  `state-root claim`), with no Runtime running on that state root ("Handing a state root over"):
  it sets the state root's `installation.json` aside as `installation.json.old-<ms>` (kept) and
  writes this installation's record, which is what renaming the record aside by hand and letting
  this installation's next start claim the state root did, and what a person did by hand before
  stays right. The next start records the new claim in the ledger. Ledger records that carry
  fingerprints were made with the salt of the configuration that wrote them; a state root taken
  over from a configuration with another salt keeps those records as they are, and the rescue
  row's answer says so (`fingerprints_discontinuous`).
- **The record or the identity file cannot be read**: read the `caused_by` link; free the file
  when another program holds it, or put it back from a backup of the same installation or the same
  state root. A damaged `installation.json` is set aside and replaced by the same command.
- **The identity file is corrupt and there is no backup of it**: the same command with
  `--new-identity`, with no Runtime running. It sets both files aside, the corrupt
  `install\identity.json` as `identity.json.old-<ms>` and the state root's `installation.json` as
  `installation.json.old-<ms>` (renamed, never deleted), mints a new number and claims the state
  root with it; the next start records a new fact. What this gives up is the old number, as the
  same done by hand before gave it up: the installation is a new one from then on, so another
  state root that recorded the old number refuses it as another installation's and needs the same
  command, and the two set-aside files are what is left of the old identity and the old claim.
  Nothing of the ledger is lost: its records (the old claim's fact included), the owner journal's
  earlier records and the integrity key are not touched.

## Codes

| Code | Category | Values | Where |
|---|---|---|---|
| `state_root_installation_mismatch` | fatal | `path` (the state root), `at_unix_ms` (the record's claim time) | the check and the claim; `unlock-owner`, `ledger-maintenance`, check-config |
| `state_root_installation_record_invalid` | fatal | `path` (the file); the read failure as `caused_by` | the same; `claim-state-root` for an identity file without `--new-identity` |
| `state_root_claim_failed` | fatal | `path` (the file); the operating system's failure as `caused_by` | the claim; `claim-state-root` (also without an installation root, `path` the state root) |
| `state_root_claimed` | info | `path` (the state root) | actingd's daemon line of a claiming start; `claim-state-root`'s success |

Operations: `check_state_root_installation` (the read-only check, and `claim-state-root`'s refusals
before the owner is taken), `record_state_root_installation` (the claim, and the hand-over).
check-config's stage: `config_state_root`. S12 adds no code: `claim-state-root` also answers the
existing `invalid_arguments` (its command line; `--new-identity` while the identity file can be
read), `owner_conflict` and `owner_resource_unconfirmed` (the owner, as a start's), and the
configuration's codes at stage `config_load`. Its rescue row adds the location
`install_state_root_claim`, the `install_refusal` `state_root_claim_unanswered` (actingd gave no
report) and the `install_caveat` `fingerprints_discontinuous`.

## Interfaces

None raised ([component-interfaces.md](component-interfaces.md)): the two files are no
configuration and no ledger shape; the fact is a new key of an existing record type; the codes,
operations and the stage are catalog additions; the tag list's default grows by one entry (no key,
no revision, as `host.agent_wake` and `host.capped_episodes` joined it). check-config's report gains
one member; its schema is unchanged. S12 raises none either: `actingd claim-state-root` is a new
subcommand with a new report schema, which no older component reads; the fact's row gains an
optional member, `previous_claimed_at_unix_ms`, a value of an existing record type that no reader
checks (a `record_list` row of scalars); the owner journal and both files keep their shapes; the
rescue row, its location and its two words are catalog additions; the rescue module's `op status`
gains a switch, `state_root_installation`, and with it a read-only member `state_root`, its
answer without the switch unchanged (`actingctl install status` is unchanged).

**At deploy.** The first start of this release on an installation mints `install\identity.json`,
writes the state root's `installation.json`, records one fact and writes one daemon line; nothing
else of the start changes. A build before this slice ignores both files; forward again they
match. S12 changes nothing at a start until a state root is handed over; a build before S12 never
records the new claim after a hand-over (it records nothing while it finds a fact), so the claim's
fact waits for the next start of this release.
