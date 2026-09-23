# actingd owner unlock

`actingd unlock-owner` is the offline maintenance command for a state root whose
last Runtime owner exited while device resources were open. Startup refuses such
a root with `owner_resource_unconfirmed`: the last record of the owner journal
(`<state_root>/owner.lock`, schema `actingcommand.runtime-owner.v2`) carries the
resource disposition `in_use` or `unconfirmed`, and nothing proves those
resources were released. The command records the operator's confirmation that
they were. It never deletes or rewrites the journal, which remains the native
proof the ledger checks at every start (`contracts/ledger-store.md`, "Proven
prior-epoch scope close").

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
Argument errors print no result object: `unlock_owner_usage_invalid` (more than
five arguments after the subcommand), `unlock_owner_option_invalid` (an unknown
or repeated option, a dangling flag or an empty value),
`unlock_owner_config_missing`, `unlock_owner_actor_missing` and
`unlock_owner_actor_invalid` follow the normal `FATAL actingd: <code>` line with
exit code 1.

## Order

Every refusal below returns before the command writes anything.

1. `owner.lock` is opened without being created and locked with the same
   exclusive OS lock the daemon holds while it runs. A missing file is
   `owner_unlock_not_required`; a held lock is `owner_unlock_daemon_active`.
   Process liveness is decided by the lock alone: no pid and no
   `runtime-info.json` is read.
2. The journal is read with the startup reader: the same complete-read
   validation, and the same recovery that truncates an incomplete final line
   left by a crash mid-append. Unless the last record is a v2 record with
   disposition `in_use` or `unconfirmed`, the result is
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
   lock released.

No new owner epoch is acquired. The confirmed epoch stays the last journal
record, so the next ordinary start takes it over automatically: it records
`runtime.takeover`, imports the epoch's close evidence and closes its
remaining scopes exactly as after a crash whose resources were confirmed
closed. No `actingctl` subcommand exists; a UI or operator spawns this command
directly.

## Result

Exactly one JSON object is written to stdout on both outcomes.

```json
{"schema_version":"actingcommand.actingd.unlock-owner.v1","status":"ok","owner_epoch":"epoch_<32 hex>","previous_resource_disposition":"in_use","actor":"operator-a","revision":4}
```

`owner_epoch` is the unlocked epoch, `previous_resource_disposition` its
disposition before the unlock (`in_use` or `unconfirmed`) and `revision` the
revision of the appended journal record.

```json
{"schema_version":"actingcommand.actingd.unlock-owner.v1","status":"failed","error":{"code":"owner_unlock_daemon_active","stage":"journal"},"journal_appended":false}
```

`error.stage` is `load` (the configuration codes of `ledger-maintenance`, for
example `config_decode_failed` or `maintenance_config_invalid`), `journal`
(`owner_unlock_daemon_active`, `owner_unlock_not_required`,
`owner_unlock_confirmation_missing`, or an owner journal read or write code
such as `owner_record_invalid` or `owner_write_failed`) or `ledger` (the ledger,
database or artifact store code, for example `ledger_migration_required` or
`artifact_retention_recovery_pending`). `journal_appended` is `true` only at
stage `ledger`: the unlock record is durable and the next start takes the epoch
over, but the `owner.unlock` fact was not recorded.

## Exit code

`0` only when `status` is `ok`. A failed unlock prints its JSON object, then the
normal `FATAL actingd: <message>` line on stderr, and exits `1`.
