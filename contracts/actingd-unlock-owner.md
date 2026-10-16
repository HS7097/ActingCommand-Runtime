# actingd owner unlock

`actingd unlock-owner` is the offline maintenance command for a state root whose
last Runtime owner left device resources open. Startup refuses such a root with
`owner_resource_unconfirmed` when the last record of the owner journal
(`<state_root>/owner.lock`, schema `actingcommand.runtime-owner.v2`) carries the
resource disposition `in_use` or `unconfirmed`, nothing proves those resources
were released, and the recorded owner process is still running or cannot be
probed; a dead owner process is released by startup itself (see "When startup
releases the owner itself"). The command records the operator's confirmation
that the resources were released. The command never deletes or rewrites the
journal. The owner guard's checkpoint fold (`contracts/owner-journal.md`) keeps
everything of the native proof the ledger checks at every start
(`contracts/ledger-store.md`, "Prior-epoch scope close (proven or unproven)").

## When startup releases the owner itself

Before refusing, startup probes the process named by the last record's `pid`
and `started_at_unix_ms` (Workflow #315 B2c-2):

- dead: no running process has the pid (none exists, or the one that exists has
  exited), or the running one was created more than 2 s after
  `started_at_unix_ms` (a reused pid, which can only have been created after
  the recorded start). Startup takes the epoch over with the
  owner semantics of `--confirm-resources-released`: it appends only its own new
  active record (no `confirmed_closed` record is written for the old epoch),
  takes over the old epoch's active instances as after a crash, appends one
  `runtime.lifecycle_observed` fact with phase
  `prior_epoch_owner_released_by_exit` carrying `pid`, `started_at_unix_ms` and
  `last_disposition`, and writes the daemon lines of `owner_released` (info,
  values `owner_pid` and `at_unix_ms`, the recorded start) on stderr after
  `ledger_opened` and before `runtime_ready` (Workflow #378 A6a): the developer line
  `INFO actingd: owner_released owner_resources_released_by_exit pid=<pid> started_at_unix_ms=<ms> last_disposition=<in_use|unconfirmed>`
  and `OUTCOME actingd: <envelope>`. This command is not needed.
- alive: the pid is running with a creation time at most 2 s after
  `started_at_unix_ms`, however much earlier (the owner writes
  `started_at_unix_ms` only after loading its configuration and resources). The
  refusal stays; its outcome carries `owner_pid` and `owner_probe_state`
  `running`, and its developer text contains `: pid <pid> alive`.
- unknown: the probe cannot decide, for example because the query is refused
  (a protected process) or the exit status or creation time cannot be read. The
  refusal stays; its outcome carries `owner_pid`, `owner_probe_state` `unknown`
  and the probe's reason as its `caused_by` link, and its developer text contains
  `: probe unknown: <reason>`. A probe failure is never read as dead.

The automatic release concerns the owner guard only and is not itself close
evidence for the ledger. The same start imports the released epoch into the
ledger as unproven with reason `process_exit_only` (`proof_missing` when its
journal block is inconsistent or recorded `confirmed_closed` before) and closes
its remaining scopes (`contracts/ledger-store.md`, "Prior-epoch scope close
(proven or unproven)"); those closes serve retention only. Device writes still
require a live scheduler lease: taken over instances go through the takeover
cool-down, reconnection and a new lease as before. In the alive and unknown
cases this command remains required.

## Invocation

```text
actingd unlock-owner --config <path> --actor <name> --confirm-resources-released
```

Options may appear in any order. `--config` names the daemon configuration; only
`state_root` and `secret_fingerprint_salt` are used, as for
`ledger-maintenance`. `--actor` names the operator and must be a path-safe
string: 1 to 256 bytes, no `/`, `\`, `:`, `..` or control character, not `.`
and not an absolute path. `--confirm-resources-released` is the operator's
statement that the device resources of the retained epoch are released.
Argument errors print the report with their outcome alone (no `stage`, no
`journal_appended`; see "Result"): `unlock_owner_usage_invalid` (more than
five arguments after the subcommand), `unlock_owner_option_invalid` (an unknown
or repeated option, a dangling flag or an empty value),
`unlock_owner_config_missing`, `unlock_owner_actor_missing` and
`unlock_owner_actor_invalid`, each with its developer line
`ERROR actingd: <code> <text>` on stderr and exit code 1.

## Order

Every refusal below returns before the command writes anything. Recovery writes
of the kind step 2 describes are not writes of this command.

1. `owner.lock` is opened without being created and locked with the same
   exclusive OS lock the daemon holds while it runs. A missing file is
   `owner_unlock_not_required`; a held lock is `owner_unlock_daemon_active`.
   Process liveness is decided by the lock alone: no pid and no
   `runtime-info.json` is read.
2. The journal is read with the startup reader: the same complete-read
   validation, and the same recovery that truncates an incomplete final line
   left by a crash mid-append. Before that read, a checkpoint fold that a crash
   interrupted is finished or discarded from its side file, as at startup
   (`contracts/owner-journal.md`, "Side file"). Unless the last record is a v2
   record with disposition `in_use` or `unconfirmed`, the result is
   `owner_unlock_not_required`.
3. Without `--confirm-resources-released` the result is
   `owner_unlock_confirmation_missing`. This check follows the first two so the
   operator learns whether an unlock is needed at all.
4. One record is appended to the same epoch: the last record with its revision
   plus one and disposition `confirmed_closed`. Epoch, pid, `started_at_unix_ms`
   and active instances are kept; the record stays active and has no
   `closed_at_unix_ms`. This record is the epoch's positive close evidence.
5. Still under the owner lock, the existing ledger is opened (existing storage
   only; a writer left by the exited owner goes through the ledger's own
   stale-owner recovery) and one `cli.command` fact is appended with action
   `owner.unlock`, origin source `cli`, module `runtime`, actor `user`, and
   client payload kind `owner_unlock` carrying `owner_epoch`,
   `previous_resource_disposition` and `actor`. The ledger is closed and the
   lock released. As at startup, the ledger is first classified by its keyed
   metadata row and then verified once by the writer open, under the same bounds.

No new owner epoch is acquired. The confirmed epoch stays the last journal
record, so the next ordinary start takes it over automatically: it records
`runtime.takeover`, imports the epoch's close evidence and closes its
remaining scopes exactly as after a crash whose resources were confirmed
closed. No `actingctl` subcommand exists; a UI or operator spawns this command
directly.

## Result

Exactly one JSON line is written to stdout on every exit, the v2 report
(Workflow #378 A6a): `{schema_version, outcome, …}`. `outcome` is the full
`actingcommand.outcome.v1` envelope; with `ACTINGCOMMAND_DETAIL=codes` in the
environment it keeps only its registered names (`"detail":"codes"`). v1's
`status` and `error {code, stage}` are gone.

```json
{"schema_version":"actingcommand.actingd.unlock-owner.v2","outcome":{"schema_version":"actingcommand.outcome.v1","code":"unlock_owner_completed","category":"success","values":{}},"owner_epoch":"epoch_<32 hex>","previous_resource_disposition":"in_use","actor":"operator-a","revision":4}
```

`unlock_owner_completed` carries the receipt: `owner_epoch` is the unlocked
epoch, `previous_resource_disposition` its disposition before the unlock
(`in_use` or `unconfirmed`) and `revision` the revision of the appended journal
record.

```json
{"schema_version":"actingcommand.actingd.unlock-owner.v2","outcome":{"schema_version":"actingcommand.outcome.v1","code":"owner_unlock_daemon_active","category":"error","values":{"operation":"unlock_owner_file","stage":"unlock_owner_file"}},"journal_appended":false}
```

A failed unlock is the failure's outcome, whose `stage` value is a registered
location, and `journal_appended`. The stage is `config_load` (the configuration
codes of `ledger-maintenance`, for example `config_decode_failed` or
`maintenance_config_invalid`), `unlock_owner_file` (`owner_unlock_daemon_active`,
`owner_unlock_not_required`, `owner_unlock_confirmation_missing`, or an owner
journal read or write code such as `owner_record_invalid` or
`owner_write_failed`) or `unlock_owner_ledger` (the ledger, database or artifact
store code, for example `ledger_migration_required` or
`artifact_retention_recovery_pending`). `journal_appended` is `true` only at
stage `unlock_owner_ledger`: the unlock record is durable and the next start
takes the epoch over, but the `owner.unlock` fact was not recorded.

## Exit code

The process exits by the outcome's category (Workflow #378 A6a): `0` for
`unlock_owner_completed`, `1` for an error, `2` for a fatal failure. A failed
unlock also writes its developer line `<LEVEL> actingd: <code> <text>` on
stderr.
