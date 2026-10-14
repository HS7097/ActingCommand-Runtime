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
  `host_claims` sections (M387 §7.1), P-c with the `performance` section (PERF §10).
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
| `file_path` | where the file holds it: at the key, at another dotted path (none since Workflow #385 P-c made the pressure streaks keys of the `performance` section at their file members), in the `instances[]` entry with the same `instance_id` (under the member's name, or at a dotted path in it: `device_kind` ← `device.kind`), or nowhere |
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
read once at assembly). P-c's rows: the `performance` section below. Workflow #386 SR-1's rows:
the `scheduling` section below. Workflow #398 RD-1b's row: the per-instance `device_kind` (string
`emulator` or `handset`, from the entry's `device.kind`, default `none`, not settable, read once
at assembly; [device-binding.md](device-binding.md)), present only for an instance that names
`device`. A manifest key the table does not have fails the start (`config_key_unknown`). The
manifest lists the exact keys in row order (`recovery_ladder`, `host_claims`, `standard_packs`,
`performance`, `scheduling`), then each instance's per-instance keys together.

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
| `recovery_ladder.triggers` | list of trigger items `{id, stages, classes?, codes?, count?, window_ms?, start?, start_rung?, respect_cooldown?}` | `navigation_exhausted` (`start_rung` 3), `pack_logic`, `stability_burst`, `preparation_backend` |
| `recovery_ladder.exclude_codes` | list of codes or `prefix*` | `contained_task_prerequisite_*`, `contained_task_return_home_entry_unmatched` |
| `recovery_ladder.rules.direct_runs` | `exempt` \| `trigger` | `exempt` |
| `recovery_ladder.rules.preparation_superseded` | `suppress` \| `keep` | `suppress` |
| `recovery_ladder.rules.already_running` | `suppress` \| `queue` | `suppress` |
| `recovery_ladder.rules.cooldown_opens_on` | `admitted` \| `triggered` | `admitted` |
| `recovery_ladder.rules.entry_unavailable` | `skip` \| `fail` | `skip` |
| `recovery_ladder.rungs` | list of `{action, <bounds>}`, one to three, no action twice | return home (`configured_package_deadline_ms` 1 800 000), application restart (`adb_probe_ms` 30 000, `entry_hold_ms` 90 000), emulator restart (`stop_bound_ms` 210 000, `start_bound_ms` 220 000, `readiness_window_ms` 120 000, `readiness_backoff_ms` [5 000, 10 000, 20 000], `readiness_poll_ms` 500) |
| `recovery_ladder.hold.claim_ttl_ms` | milliseconds | `120000` |
| `recovery_ladder.hold.margin_ms` | milliseconds | `5000` |

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

The digest a ladder records (`recovery_ladder_started` `config_digest`) is the SHA-256 of the
canonical JSON of the section's values in effect when the ladder was admitted; a ladder climbs by
that snapshot to its end, its wait in the queue included, and the next trigger reads the values
in effect then (M387 §4.7, §5.8).

## The `host_claims` section (D1c)

Frozen model M387 v1.2 §2.1 row 8, §4.3: what every Runtime-held key (a recovery ladder's, a
startup package's) leaves alone before a step. Its own section, since both renew through the same
function.

| Key | Type | Default (`actingd.defaults.json`) |
|---|---|---|
| `host_claims.renewal_slack_ms` | milliseconds, settable (read at the next renewal) | `1000` |

Before a step a Runtime-held key renews to the step's bound (capped at a task's longest deadline
plus the heartbeat bound, as every claim TTL), unless its remaining time plus
`renewal_slack_ms` already covers that bound. The value is read at each renewal; a value that is
not milliseconds is refused (`config_value_invalid`, `wrong_type`).

## The `standard_packs` section (Workflow #373 NAV-6; moved by D1c)

| Key | Type | Default |
|---|---|---|
| `standard_packs.root` | string (a path), not settable (read once at assembly) | none: without it no standard-pack root is read |

The directory that holds one root per game (`<root>/<game>/`), written once by the installer. A
relative path resolves against the configuration file's directory; an empty one is refused.
D1c moves the section's structure from actingd's own configuration into this crate (NAV-6
deviation 5); the file member and its meaning are unchanged, and `config.parameters.standard_packs`
reports it as the file writes it when the file names it. Changing it needs a commit and a
restart.

## The performance section (Workflow #385 P-c)

Frozen model PERF v1.4.2 §10 (`contracts/v012-register.md` §5): every key of performance pacing,
parsed, checked and defaulted from v0.12.0 on, though most are read by later slices (order-v012
R4). The defaults are PERF §10's, all in `actingd.defaults.json`; a configuration names only what
it changes, so a key it leaves out takes the running release's default. Neither the example
configuration nor any installer writes a performance key.

| Subsection (file) | Keys | Read by |
|---|---|---|
| `performance.monitor.*`, `performance.load.*`, `performance.pressure.*`, `performance.pressure_start_samples`, `performance.pressure_end_samples` (`src/performance/monitor.rs`) | PERF §10.1, 15 keys: sampling period (1 000..=5 000 ms), context window, summary interval, sample bounds, process coverage, failures, control freshness, stutter gap, heavy load threshold, the two pressure streaks (1..=30), `pressure.thresholds` and `pressure.severity` (objects), `pressure.gpu_aggregate` (`max_engine_type` \| `sum`) | the streaks: the start-up assembly, as before (their file members are unchanged); the rest: P-m |
| `performance.responsiveness.*` (`src/performance/responsiveness.rs`) | PERF §10.2, 37 keys: window, smoothing, staleness, the timer and message probes, the capture, touch, recognition and page-settle signals (per-backend initial baselines are objects with a `default` member), the baseline | P-m |
| `performance.pacing.*` (`src/performance/pacing.rs`) | PERF §10.3, 26 keys: `mode` (`observe` \| `pace` \| `full`, default `observe` in every release), `interpolate`, `curve` (a list of at most 70 points), the factor cap, hysteresis and sustains, pause timing, waits and gaps, `pressure_floor.*`, `tax_exempt_from_stage`, `heavy_dispatch_limit.normal` / `.paused`, `clock_jump_ms`, `hold.*`, `pause_probe.*` | `heavy_dispatch_limit.*` and `clock_jump_ms`: the performance controller, handed over by the start-up assembly (P-c; before, code constants); the rest: P-m, P-k, P-s, P-h |
| `performance.calibration.*` (`src/performance/calibration.rs`) | PERF §10.4, 19 keys: `apply`, `band_bp`, `trouble_codes` (a list of catalog codes), the learning period, `learning_max_stage` (S0..=S4), the backstop, interval, minimum steps, `tolerance.s0` / `.s1` / `.s2` / `.stop`, `bounds.s1` to `bounds.s4` (`[min, max]` basis points; PERF's `bounds.S1`, lowercase since a key is a token), `max_shift_bp`, `pressure_max_stage` | P-a |

- **Settable:** `performance.pacing.curve` and `performance.pacing.hold.*` (M387 §5.5); every other
  key is read once at assembly and changes by the file, a commit and a restart. `mode` in
  particular is a person's configuration, never an instruction (PERF §4.6).
- **Whole-section check** (`perf_config_invalid`, `config_key` and `config_value_problem`; through
  an instruction `config_value_invalid` with the same key and problem): the curve has at most
  **70 points** (`out_of_range` beyond; coordinator ruling 2026-10-14: 70 is the largest count
  whose serialized JSON fits the 8 KiB `ConfigureSettings` value limit for every curve these
  rules allow, the longest being 117 × points − 1 bytes, 8 189 for 70 and 8 306 for 71, so any
  curve a file may hold can also be sent by an instruction) and runs from
  `r_bp` 10 000 down to 0, strictly falling; each point is `{r_bp, stage, factor_milli,
  extra_retries, share_milli}`, its factor within 1 000 and `max_factor_milli`, its share within
  0 and 1 000, its stage S0 to S3; as R falls the factor never falls, the share never rises and
  the stage never steps back; no two neighbours drop the share from 1 000 straight to 0 (PERF
  §4.1). `tolerance` rises strictly; each `bounds.sN` has min below max, and a deeper stage's
  bounds lie no higher than the stage before it; `pressure_max_stage` is S0 to S2;
  `trouble_codes` are active catalog codes (`code_unknown`, `code_retired`) (PERF §10.4). The
  context window and summary interval are at least the sampling period; each threshold pair
  ends below where it starts; an elevated severity is not above the critical one; each
  per-backend baseline has `default` and positive values; the baseline window holds at least
  its minimum and its clamp floor is not above its ceiling; `pressure_floor.kinds` are known
  pressure kinds, each once; `heavy_dispatch_limit.paused` is not above `.normal`. A value of the
  wrong type or outside a key's range is `config_value_invalid`. At start and in `check-config`
  the refusal comes before the ledger opens, exit code 1.

## The `scheduling` section (Workflow #386 SR-1)

Frozen scheduling model v1.4.2 §9 and [v012-register.md](v012-register.md) §5: every key is
parsed, checked and defaulted from v0.12.0 on, all settable, default `file`; the Runtime reads
`scheduling.recognition.sample_timeout_retries` at each step of a contained run (the kernel's
`step_pacing()`), the others take effect with the 0.12.x slices that read them. Their meaning is
in [policy-suspension.md](policy-suspension.md) (`recognition`, `rerun`),
[cost-commit.md](cost-commit.md) (`cost_commit`) and [scheduling-blackout.md](scheduling-blackout.md)
(`blackout`); `feature_lock.*` (how long a recorded locked feature stays valid, and the codes
that end a run on one) is read by slices SR-5a and SR-5b, `not_run.diagnose_rows` (the most rows
`ac_diagnose` lists under `not_run`) by SR-6b.

| Key | Type | Default (`actingd.defaults.json`) |
|---|---|---|
| `scheduling.recognition.sample_timeout_retries` | integer `0..=4294967295` | `2` |
| `scheduling.rerun.condition_codes` | list of codes the catalog lists and has not retired | the model's eight codes |
| `scheduling.rerun.max_per_occurrence` | integer `0..=4294967295` | `2` |
| `scheduling.rerun.resume_stage` | `s0`…`s4` (vocabulary `pacing_stage`) | `s2` |
| `scheduling.rerun.resume_settle_ms` | milliseconds | `60000` |
| `scheduling.rerun.promote`, `.refund_counts` | boolean | `true`, `true` |
| `scheduling.rerun.expires` | `window_end` | `window_end` |
| `scheduling.cost_commit.promote` | boolean | `true` |
| `scheduling.cost_commit.max_stage` | `s0`…`s4` | `s2` |
| `scheduling.cost_commit.margin_ms` | milliseconds | `60000` |
| `scheduling.blackout.host` | list of host blackout entries | `[]` |
| `scheduling.blackout.quiet_margin_ms`, `.hard_quiet_ms` | milliseconds | `60000`, `0` |
| `scheduling.blackout.stop_in_flight_at_start` | boolean | `true` |
| `scheduling.feature_lock.locked_valid_for_ms` | milliseconds | `86400000` |
| `scheduling.feature_lock.codes` | list of codes, as `rerun.condition_codes` | `["contained_task_feature_locked"]` |
| `scheduling.not_run.diagnose_rows` | integer `0..=4294967295` | `10` |
| `scheduling.random_delay.enabled` (Workflow #405) | boolean | `false` |
| `scheduling.random_delay.min_ms`, `.max_ms` (Workflow #405) | milliseconds `0..=86400000` | `60000`, `3600000` |

The section is checked whole at start, in `check-config` and after every instruction: a code
list entry the catalog does not list is `config_value_invalid` with `code_unknown`, a retired
one `code_retired`; a host blackout entry that breaks a rule of
[scheduling-blackout.md](scheduling-blackout.md) refuses a start and `check-config` with
`blackout_config_invalid` (`field`, and `blackout_id` when the entry has a usable id: one to 256
bytes, no control character) and an instruction with `config_value_invalid` (`section_check`);
`random_delay.min_ms` above `random_delay.max_ms` is `config_value_invalid` (`section_check`) for
`min_ms`.

**Per instance (Workflow #405).** `instance.<instance id>.scheduling.random_delay.enabled`,
`.min_ms` and `.max_ms`, in the file the entry's `scheduling.random_delay` member, settable,
default `section:scheduling.random_delay.<member>`. The pair in effect (each side the
instance's own or the section's) is checked: `min_ms` above `max_ms` is `config_value_invalid`
(`section_check`) for the instance's `min_ms` key, so a change of one side alone is checked
against the other. No slice reads them before #405.

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
`recovery_ladder_config_invalid` and `recovery_ladder_code_unclassified` (D1c emits them),
`blackout_config_invalid` (registered by SR-0; host blackout entries are parsed in this crate,
so the code moved here; SR-1 emits it at start and in `check-config`) and `perf_config_invalid`
(Workflow #385 P-c; domain `perf_`). Owner `runtime-host`:
`host_config_overrides_fact_invalid`, `host_config_overrides_persist_failed`,
`host_config_overrides_fact_clock_overflow`, `host_config_value_type_invalid`. Owner `contract`:
`invalid_configure_settings`. Vocabulary `config_value_problem` (source
`crates/runtime-config/src/codes.rs`).
Every one is ladder class `none` (the Runtime's own configuration, decision-table row 7).
