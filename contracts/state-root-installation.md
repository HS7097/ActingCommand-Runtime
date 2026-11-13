# State root installation record

Workflow #453 S11 (Runtime 0.12.60; register [v012-register.md](v012-register.md) §1.358). The
owner approved board #453; the coordinator's rulings on the board replace the design survey's
identity by the configuration's salt. Before this slice nothing in a state root said which
installation it belongs to, so a configuration that names another installation's state root
started on it whenever that installation was not running. Since this slice a Runtime started from
an installation root records, in the state root, which installation claimed it, and refuses to
start on another installation's state root.

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
  number. Nothing else writes, moves or renames it; the installer leaves it as it is ("The
  installer" below).
- What one installation is: whatever carries the same file. A moved installation folder, a
  restored backup of it and two builds of one installation carry the same file and are one
  installation. A copy of a whole installation root carries it too and is the same installation:
  that limit is accepted and has no rule.

## Which root is the installation root

The **installation root** of a program that writes the state root (a start, `actingd
unlock-owner`, every `actingd ledger-maintenance` pass) is the directory of its
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
refuses the start, `actingd unlock-owner`, every `actingd ledger-maintenance` pass and
`actingd check-config` with `state_root_installation_record_invalid` (`path` that
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
  deletes it. A later slice brings the command that hands a state root over to another
  installation (it keeps the old record); this slice has none.
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
  record, once; any other start records nothing.

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

Only a start claims, **under the owner lock**, right after it took it
(`OwnerGuard::acquire`, which wrote the start's owner record):

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

## Who checks and who does not

- **A start** checks and claims (above).
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
tree comparisons; none refuses an unknown file and none needed a change. No new `actingctl`
command comes with this slice, and no rescue operation runs any of it on a production root.

## When a start is refused

The Runtime stays down (fatal) and the start's log names the configured state root and the time
the record's installation claimed it.

- **The configuration is wrong** (the usual case: a configuration copied from another
  installation root, or a `state_root` edited by hand): correct `state_root` so that it names this
  installation's own state root, or put the previous configuration file back. Nothing was written
  to either installation.
- **This installation is meant to own that state root**: a later release brings the command that
  hands the state root over (it keeps the old record). Until it exists the only other way is by
  hand: with no Runtime running on that state root, rename its `installation.json` aside (keep
  it), and this installation's next start claims the state root. Ledger records that carry
  fingerprints were made with the salt of the configuration that wrote them; a state root taken
  over from a configuration with another salt keeps those records as they are.
- **The record or the identity file cannot be read**: read the `caused_by` link; free the file
  when another program holds it, or put it back from a backup of the same installation or the same
  state root. A damaged `installation.json` can also be renamed aside by hand as above.
- **The identity file is corrupt and there is no backup of it**: until the hand-over command of
  the next slice exists, the only way out is a hand-over done by hand. With no Runtime running,
  set both files aside: rename the corrupt `install\identity.json` and the state root's
  `installation.json` (rename them, never delete them). The next start then mints a new number
  and claims the state root with it, with its daemon line and a new fact. This is a hand-over
  done by hand, and what it gives up is the old number: the installation is a new one from then
  on, so another state root that recorded the old number refuses it as another installation's
  and needs the same by hand, and the two set-aside files are what is left of the old identity
  and the old claim. Nothing of the ledger is lost: its records (the old claim's fact
  included), the owner journal and the integrity key are not touched.

## Codes

| Code | Category | Values | Where |
|---|---|---|---|
| `state_root_installation_mismatch` | fatal | `path` (the state root), `at_unix_ms` (the record's claim time) | the check and the claim; `unlock-owner`, `ledger-maintenance`, check-config |
| `state_root_installation_record_invalid` | fatal | `path` (the file); the read failure as `caused_by` | the same |
| `state_root_claim_failed` | fatal | `path` (the file); the operating system's failure as `caused_by` | the claim |
| `state_root_claimed` | info | `path` (the state root) | actingd's daemon line of a claiming start |

Operations: `check_state_root_installation` (the read-only check), `record_state_root_installation`
(the claim). check-config's stage: `config_state_root`.

## Interfaces

None raised ([component-interfaces.md](component-interfaces.md)): the two files are no
configuration and no ledger shape; the fact is a new key of an existing record type; the codes,
operations and the stage are catalog additions; the tag list's default grows by one entry (no key,
no revision, as `host.agent_wake` and `host.capped_episodes` joined it). check-config's report gains
one member; its schema is unchanged.

**At deploy.** The first start of this release on an installation mints `install\identity.json`,
writes the state root's `installation.json`, records one fact and writes one daemon line; nothing
else of the start changes. A build before this slice ignores both files; forward again they
match.
