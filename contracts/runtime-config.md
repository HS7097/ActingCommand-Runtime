# Runtime configuration mechanism

Workflow #387 D1b (frozen model M387 v1.2 §5, §6, §12). The shared mechanism every
configuration section of v0.12.0 uses: the key table, the defaults file, the file layer, the
instruction layer `ConfigureSettings`, and how both layers are recorded. The sections themselves
arrive with their slices (D1c `recovery_ladder` and `host_claims`, AO-1, P-c, SR-*, D2-S*;
[v012-register.md](v012-register.md) §5).

## Where it lives

| Part | Where |
|---|---|
| Section structures, the key table, merge, whole-section checks, the defaults file, the instruction layer | crate `crates/runtime-config` (`actingcommand-runtime-config`), below runtime-host, depending on the contract only; one source file per section (`src/<section>.rs`, a shared section `src/<section>/<subsection>.rs`) |
| Reading the files, `check-config`, the start-up manifest | actingd |
| The configuration in effect, `ConfigureSettings`, restoring instruction values at start | runtime-host (`host/runtime_config.rs`) |

## Layers

| Layer | What | Who changes it | Takes effect |
|---|---|---|---|
| File | `actingd.defaults.json` ⊕ the committed `actingd.config.json` | the person, through `acsetup --commit-config` | at the next start |
| Instruction | `host.config_overrides` | the three counters (`actingctl config set\|clear`, MCP `ac_config_set`, the console) through `ConfigureSettings` | at once, for a settable key |

- **Defaults file.** `distribution/windows/actingd.defaults.json`, compiled into the program and
  shipped in the Runtime artifact beside the example configuration; the exact-SHA build checks
  that the shipped file's SHA-256 is the `defaults_sha256` `check-config` reports. It holds
  exactly the keys whose `default` is `file`, parsed by `DefaultsFile` (no required top-level
  member; a member without parsing code is refused). D1c fills it with the `recovery_ladder` and
  `host_claims` sections (M387 §7.1).
- **Merge.** A key the file names is `explicit`, otherwise `default` (from the defaults file, the
  section-level key or code). A list or object the file names replaces the default whole. The
  example configuration stays a skeleton: a configuration names only the keys it changes. A key
  whose default is a section-level key (`section:<key>`) and that the file does not name follows
  that key's value in effect, its instruction value included (D1c); an instruction on the
  section-level key records the dependent section again. A host started without a manifest
  reads the defaults file's values.
- **Checks.** After the merge each section passes its whole-section check, at start (a start and
  `check-config` refuse the same way, with the failure's own code) and after every instruction.
  A check that needs a fact of another crate reads it from the check context the caller fills
  in: the scheduler's `maximum_client_heartbeat_interval_ms` and (D1c) the device layer's own
  bounds of the application stop, the emulator Stop and the emulator Start
  (`recovery_ladder_device_bounds`). A section's broken rule refuses a start or `check-config`
  with the section's own code; the same values sent by an instruction are refused as
  `config_value_invalid` with the same `config_key` and `config_value_problem`.

## Key table

One row per key ([v012-register.md](v012-register.md) §5 lists the sections):

| Column | Meaning |
|---|---|
| `key` | as `config.parameters` names it; a per-instance key is `instance.<instance id>.<member>` |
| `file_path` | where the file holds it: at the key, at another dotted path (`performance_monitor.pressure_*` ← `performance.pressure_*`), in the `instances[]` entry with the same `instance_id`, or nowhere |
| `type` | boolean, integer, milliseconds, string, decimal, list, object (decimal, list and object are reported as canonical JSON text) |
| `range` | allowed values (schema, not a threshold) |
| `default` | `file`, `none`, `section:<key>` or `code` (a key whose default no slice has moved yet) |
| `settable` | whether an instruction may change it while the Runtime runs; `false` names why: used before the ledger opens, read once at assembly, no file member, or derived |
| `reader`, `since` | the slice that reads it; the release from which it has this row |

D1b's rows: every key the start-up manifest reported before it (none settable, M387 §12
question 2: change it in the file, commit, restart) and the per-instance `stuck_recovery`
(boolean) and `stuck_recovery_cooldown_secs` (integer 1..=86400), settable, default
`section:recovery_ladder.enabled` and `section:recovery_ladder.cooldown_secs` (D1c). D1c's rows:
the `recovery_ladder` and `host_claims` sections below (all settable, default `file`) and
`standard_packs.root` (Workflow #373 NAV-6, moved here: string, default `none`, not settable,
read once at assembly). A manifest key the table does not have fails the start
(`config_key_unknown`). The manifest lists the exact keys in row order (`recovery_ladder`,
`host_claims`, `standard_packs`), then each instance's per-instance keys together.

## The recovery ladder section (D1c)

Frozen model M387 v1.2 §3, §4, §7; [emulator-control.md](emulator-control.md), "Stuck-recovery
ladder", describes what each key does. Every key can change while the Runtime runs; a ladder
climbs by the section it was admitted with (§4.7).

| Key | Type | Default (`actingd.defaults.json`) |
|---|---|---|
| `recovery_ladder.enabled` | boolean | `true` |
| `recovery_ladder.cooldown_secs` | integer 1..=86400 | `600` |
| `recovery_ladder.trigger_defaults.start` | `immediately` \| `after_performance_release` | `immediately` |
| `recovery_ladder.trigger_defaults.start_rung` | integer 1..=3 | `1` |
| `recovery_ladder.trigger_defaults.respect_cooldown` | boolean | `true` |
| `recovery_ladder.triggers` | list of trigger items `{id, stages, classes?, codes?, count?, window_ms?, start?, start_rung?, respect_cooldown?}` | `pack_logic`, `stability_burst`, `preparation_backend` |
| `recovery_ladder.exclude_codes` | list of codes or `prefix*` | `contained_task_prerequisite_*`, `contained_task_return_home_entry_unmatched` |
| `recovery_ladder.rules.direct_runs` | `exempt` \| `trigger` | `exempt` |
| `recovery_ladder.rules.preparation_superseded` | `suppress` \| `keep` | `suppress` |
| `recovery_ladder.rules.already_running` | `suppress` \| `queue` | `suppress` |
| `recovery_ladder.rules.cooldown_opens_on` | `admitted` \| `triggered` | `admitted` |
| `recovery_ladder.rules.entry_unavailable` | `skip` \| `fail` | `skip` |
| `recovery_ladder.rungs` | list of `{action, <bounds>}`, one to three, no action twice | return home (`configured_package_deadline_ms` 1 800 000), application restart (`adb_probe_ms` 30 000, `entry_hold_ms` 90 000), emulator restart (`stop_bound_ms` 210 000, `start_bound_ms` 220 000, `readiness_window_ms` 120 000, `readiness_backoff_ms` [5 000, 10 000, 20 000], `readiness_poll_ms` 500) |
| `recovery_ladder.hold.claim_ttl_ms` | milliseconds | `120000` |
| `recovery_ladder.hold.margin_ms` | milliseconds | `5000` |
| `host_claims.renewal_slack_ms` | milliseconds | `1000` |

The whole-section check (`recovery_ladder_config_invalid`, `config_key` and
`config_value_problem`): trigger ids are tokens and unique (`duplicate_id`); an item names one or
more of `task`, `startup_preparation`, `connection_preparation`, `stability_warnings`, each once
(`stage_not_allowed`), and classes or codes (`match_missing`); every class is a ladder class and
every code or prefix is in the embedded catalog (`code_unknown`, `code_retired`); `count` and
`window_ms` are present, above 0, exactly on an item that names `stability_warnings`
(`burst_invalid`); every start rung is within the rungs (`start_rung_out_of_range`); the rungs
are one to three known actions, each once, each element naming only its own action's bounds
(`rungs_invalid`); `configured_package_deadline_ms` is within a task's longest deadline,
`adb_probe_ms` at least 1 000, `entry_hold_ms` at least the probe and the device's application
stop, `stop_bound_ms` and `start_bound_ms` at least the device's own, `readiness_backoff_ms`
non-empty without 0, `readiness_window_ms` at least its first wait and `readiness_poll_ms`
within 100 and the window (`bound_out_of_range`); every other bound plus `hold.margin_ms` within
the longest renewal, a task's longest deadline plus the heartbeat bound (`renewal_cap_exceeded`);
`hold.claim_ttl_ms` above the heartbeat bound, within the longest renewal and not below the
margin (`claim_ttl_out_of_range`). Elements a list leaves out take their schema values: a
trigger item's `start`, `start_rung` and `respect_cooldown` from `trigger_defaults`, a rung's
bounds from the defaults file's rung of the same action.

## Facts

| Fact (scope `runtime`) | Rows | Recorded |
|---|---|---|
| `config.subsystems` | unchanged | at start |
| `config.selection` | one: `config_sha256`, `defaults_sha256` (`sha256:<hex>`), `config_generation` (decimal text, installed Runtime only) | at start |
| `config.parameters.<section>` | one per key of the section, in manifest order: `key`, `value`, `source` (`explicit`, `default`, `discovered`, `instruction`), `settable`; the section is the key's first segment, `top_level` for a key without a dot; at most 256 rows each | at start, each section; again when an instruction changes a key of it; a section an earlier start recorded and this one has not is recorded empty |
| `host.config_overrides` | one per instruction value: `config_key`, `value_json`, `base_json`, `base_source` (`explicit` or `default`), `set_unix_ms`, `set_in_owner_epoch`, `request_id`; `channel` from U-2 on | at every `ConfigureSettings`, and at start when it held rows (tagged, `tag.workbench_patterns`) |

`actingctl status --config` returns `config.subsystems`, `config.selection` when recorded and
every `config.parameters.<section>` in key order. `check-config` reads only the files: its
report carries `config_selection` and names `instruction_values` in `not_checked`.

## `ConfigureSettings`

A general instruction (Workflow #389: the three counters send it; no origin gate beyond the
client sources). Request `{changes: [{key, value?}]}`: 1 to 64 changes, each key named once;
a change without `value` (or `null`) returns the key to its file value. Malformed shape:
`invalid_configure_settings`.

All changes or none:

| Check | Refusal (Denied) |
|---|---|
| the key is in the key table (a per-instance key of a configured instance) | `config_key_unknown` |
| the key is settable | `config_key_not_settable` |
| the value has the key's type and range; no key is named twice; the changed sections pass their whole-section check | `config_value_invalid` (`config_value_problem`: `wrong_type`, `out_of_range`, `repeated_key`, `section_check`; `range_min`, `range_max` for a bounded key) |

Records: `cli.command` (or the counter's own) and `command.received`; on a refusal
`command.rejected` carrying the refusal as its outcome, and nothing else; otherwise the whole new
`host.config_overrides`, each changed `config.parameters.<section>`, then `command.validated`
(action `runtime.action`), and the values take effect. Receipt Completed with
`{settings: [{key, value, source}]}`, each changed key as it is in effect afterwards, and the
outcome `request_configure_settings_completed` (success; receipt v2, Workflow #378 A4). A
`host.config_overrides` the ledger refuses changes nothing: `command.rejected` carries
`host_config_overrides_persist_failed` with the ledger's refusal as its `caused_by` (Failed).
The receipt of a refusal in the table above, or of that failure, carries the outcome its
`command.rejected` carries on top, with `operation` (`configure_settings`,
`persist_config_overrides`), `runtime_code` (`invalid_request`, `ledger_failure`) and
`host_halted` added as values; `invalid_configure_settings` tops its Denied receipt with
`runtime_code` `invalid_request`.
Once it is recorded, a refused section record stops the Runtime; the next start restores the
recorded values, so the ledger and the values in effect never part.

## Restore at start (M387 §12 question 3: the later change wins)

Before any instance is prepared, each `host.config_overrides` row is compared with the file
layer of this start:

| Row | File layer now | Result |
|---|---|---|
| key gone, no longer settable, or value no longer fits | — | dropped, `config_override_dropped` (warning) |
| overrode an explicit value | the file still names the key with that value | kept |
| overrode an explicit value | the file changed or removed it | dropped, `config_override_superseded` (info) |
| overrode a default | the file names the key now | dropped, `config_override_superseded` (info) |
| overrode a default | the file still does not name it; the default is unchanged | kept |
| overrode a default | the file still does not name it; the default changed | kept, over the new default, `config_override_base_changed` (warning) |

The restored values then pass the whole-section check of their sections with this start's file
layer; a section that fails it drops every value restored into it (`config_override_dropped`).
Each note is a `runtime.failed` record whose outcome is the code with `config_key`, its severity
the code's category. A row not of the shape this build writes fails the start, and every later
start until the ledger is repaired (`host_config_overrides_fact_invalid`).

## Codes

Owner `runtime-config`: `config_key_unknown`, `config_key_not_settable`, `config_value_invalid`,
`config_override_superseded`, `config_override_base_changed`, `config_override_dropped`,
`recovery_ladder_config_invalid` and `recovery_ladder_code_unclassified` (D1c emits them), and
`blackout_config_invalid` (registered by SR-0; host blackout entries are parsed in this crate,
so the code moved here; SR-4a emits it). Owner `runtime-host`:
`host_config_overrides_fact_invalid`, `host_config_overrides_persist_failed`,
`host_config_overrides_fact_clock_overflow`, `host_config_value_type_invalid`. Owner `contract`:
`invalid_configure_settings`. Vocabulary `config_value_problem` (source
`crates/runtime-config/src/codes.rs`).
Every one is ladder class `none` (the Runtime's own configuration, decision-table row 7).
