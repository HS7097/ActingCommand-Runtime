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
  `host_claims` sections (M387 §7.1), P-c with the `performance` section (PERF §10), SR-1 with
  the `scheduling` section, AO-1 with the `ledger_observer` section.
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
read once at assembly); Workflow #398 RD-4 adds `recovery_ladder.rules.start_beyond_profile`,
`recovery_ladder.profiles` and `recovery_ladder.kind_profiles` to the section and the
per-instance `recovery_ladder_profile` (string, default `none`, settable). P-c's rows: the
`performance` section below. Workflow #386 SR-1's rows: the `scheduling` section below.
Workflow #383 AO-1's rows: the `ledger_observer` section below.
Workflow #398 RD-1b's row: the per-instance `device_kind` (string
`emulator` or `handset`, from the entry's `device.kind`, default `none`, not settable, read once
at assembly; [device-binding.md](device-binding.md)), present only for an instance that names
`device`. Workflow #384 D2-S2's row: the per-instance `recognition_package` (string, the location
of the instance's recognition maintenance package, from the entry's `recognition_package.package`
resolved against the configuration file's directory; a location only, never a digest; default
`none`, not settable, read once at assembly; [resource-readings.md](resource-readings.md),
"Recognition maintenance packages"), present only for an instance that names it. Workflow #369
S6a's row: the per-instance `start_emulator` (boolean, from the entry's `start_emulator`, default
`none`, so a file that does not name it leaves it off and reports no row; not settable, read once
at assembly; allowed only on a discovery-bound instance, `start_emulator_requires_discovery_binding`,
[actingd-check-config.md](actingd-check-config.md)); the daemon acts on it from S6c on. Workflow
#389 U-2's rows: the top-level `lab_module_authoring` and `lab_module_debugging` (boolean, default
`file`, `false` in `actingd.defaults.json`, not settable, read once at assembly), the Lab's two
options ([runtime-interface.md](runtime-interface.md), "Lab options"); a file names one
only when it is `true`, and only the installer's Lab transaction writes them. Until U-6 they are
read and reported only: nothing is refused by them. Workflow #390 DISC-1's rows: the
`startup_discovery` section below (default `file`, not settable, read once at assembly). A
manifest key the table does not have fails
the start (`config_key_unknown`). The manifest lists the exact keys in row order
(`recovery_ladder`, `host_claims`, `standard_packs`, `performance`, `scheduling`,
`ledger_observer`, the two Lab options), then each instance's per-instance keys together.

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
| `recovery_ladder.triggers` | list of trigger items `{id, stages, classes?, codes?, count?, window_ms?, start?, start_rung?, respect_cooldown?}` | `navigation_exhausted` (`start_rung` 3), `pack_logic`, `stability_burst`, `preparation_backend`, `task_capture_backend` |
| `recovery_ladder.exclude_codes` | list of codes or `prefix*` | `contained_task_prerequisite_*`, `contained_task_return_home_entry_unmatched` |
| `recovery_ladder.rules.direct_runs` | `exempt` \| `trigger` | `exempt` |
| `recovery_ladder.rules.preparation_superseded` | `suppress` \| `keep` | `suppress` |
| `recovery_ladder.rules.already_running` | `suppress` \| `queue` | `suppress` |
| `recovery_ladder.rules.cooldown_opens_on` | `admitted` \| `triggered` | `admitted` |
| `recovery_ladder.rules.entry_unavailable` | `skip` \| `fail` | `skip` |
| `recovery_ladder.rules.start_beyond_profile` | `exhaust` \| `last_rung` \| `refuse` (Workflow #398 RD-4) | `exhaust` |
| `recovery_ladder.rungs` | list of `{action, <bounds>}`, one to three, no action twice | return home (`configured_package_deadline_ms` 1 800 000), application restart (`adb_probe_ms` 30 000, `entry_hold_ms` 90 000), emulator restart (`stop_bound_ms` 210 000, `start_bound_ms` 220 000, `readiness_window_ms` 120 000, `readiness_backoff_ms` [5 000, 10 000, 20 000], `readiness_poll_ms` 500) |
| `recovery_ladder.hold.claim_ttl_ms` | milliseconds | `120000` |
| `recovery_ladder.hold.margin_ms` | milliseconds | `5000` |
| `recovery_ladder.profiles` | object, profile name to `{rungs}` (Workflow #398 RD-4) | `handset`: return home, application restart (each rung's bounds the defaults file's of its action) |
| `recovery_ladder.kind_profiles` | object, device kind (`emulator`, `handset`) to profile name (Workflow #398 RD-4) | `handset` → `handset` |

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

Workflow #398 RD-4 (model-398 v1.1 §6.2, §6.6; [emulator-control.md](emulator-control.md),
"Ladder profiles") adds to the check: a profile name is a token (1 to 64 bytes of lowercase
letters, digits and `_`, starting with a letter; `wrong_type` on `recovery_ladder.profiles`), each
profile is `{rungs}` and its rungs pass every rule of `rungs` above, a refusal naming the profile
as `recovery_ladder.profiles.<name>`; every member of `kind_profiles` is a device kind naming one
of the profiles (`out_of_range`); each instance's `recovery_ladder_profile` names one of the
profiles (`out_of_range` on `instance.<instance id>.recovery_ladder_profile`, so a change of the
per-instance section checks this section too); and under `rules.start_beyond_profile = refuse`
every trigger item's start rung and `trigger_defaults.start_rung` lie within every profile's
rungs (`start_rung_out_of_range` on `recovery_ladder.profiles.<name>`). A profile set or object
the file names replaces the default whole, as a list does, so a file whose `profiles` leave out
`handset` is refused unless its `kind_profiles` no longer name it. The per-instance `recovery_ladder_profile` (string, the
entry's member, default `none`: reported only for an instance that names it) is settable; a
ladder takes the instance's profile with the section's snapshot when it is admitted.

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
| `performance.monitor.*`, `performance.load.*`, `performance.pressure.*`, `performance.pressure_start_samples`, `performance.pressure_end_samples` (`src/performance/monitor.rs`) | PERF §10.1, 15 keys: sampling period (1 000..=5 000 ms), context window, summary interval, sample bounds, process coverage, failures, control freshness, stutter gap, heavy load threshold, the two pressure streaks (1..=30), `pressure.thresholds` and `pressure.severity` (objects), `pressure.gpu_aggregate` (`max_engine_type` \| `sum`) | every key, read once by the start-up assembly (`MonitorSettings`, Workflow #385 PM-CFG) and handed to the performance monitor, its host sampler (process thresholds and coverage) and the performance controller (`load.heavy_threshold_milli`, which the controller's resource overlay hands to the policy evaluator, so both judge heavy work by it); the streaks keep their file members |
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
parsed, checked and defaulted from v0.12.0 on, all settable, default `file` (but
`fact_refresh.overdue_ms`, which has none); the Runtime reads
`scheduling.recognition.sample_timeout_retries` at each step of a contained run (the kernel's
`step_pacing()`), the others take effect with the 0.12.x slices that read them. Their meaning is
in [policy-suspension.md](policy-suspension.md) (`recognition`, `rerun`),
[cost-commit.md](cost-commit.md) (`cost_commit`) and [scheduling-blackout.md](scheduling-blackout.md)
(`blackout`); `feature_lock.*` (how long a recorded locked feature stays valid, and the codes
that end a run on one) is read by slices SR-5a and SR-5b, `not_run.diagnose_rows` (the most rows
`ac_diagnose` lists under `not_run`) by SR-6b. Workflow #384 D2-S2 reads
`fact_refresh.recognition_join_timeout_ms` when a successful run joins the readings it handed
to recognition ([resource-readings.md](resource-readings.md), "Kernel reading"); Workflow #384
D2-S4 reads `fact_refresh.enabled`, `min_interval_ms`, `daily_limit`, `failure_backoff_ms` and
`max_consecutive_failures` at every policy cycle, the pace of the scheduler's recognition
maintenance package dispatches ([scheduling/README.md](scheduling/README.md), "Fact refresh"); a
change takes effect from the next round. Workflow #384 D2-S5 reads `fact_refresh.overdue_ms`
at every policy cycle: how long one instance's candidates may wait for one key before the
transition recorder reports `fact_reader_overdue`; it has no default, so without it nothing is
reported, and an instruction can change it only when the configuration file names it (an unset
key has no value to replace, `config_key_unknown`). Workflow #336 PKG-S3 reads
`suspension.lift_check_min_interval_ms` at each suspension lift check: a package's content
hash is measured at most once per interval ([policy-suspension.md](policy-suspension.md),
"Lifting (R19)"). Workflow #386 SR-3 reads `pack_admission.check_in_check_config` in
`check-config`, `pack_admission.check_after_start` when the background admission after start
runs, and `pack_admission.suspend_codes` at each refusal of a scheduled run's preparation and in
both checks ([policy-suspension.md](policy-suspension.md), "Task package suspension").

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
| `scheduling.fact_refresh.enabled` (Workflow #384 D2-S4) | boolean | `true` |
| `scheduling.fact_refresh.min_interval_ms`, `.failure_backoff_ms` (Workflow #384 D2-S4) | milliseconds | `300000`, `600000` |
| `scheduling.fact_refresh.daily_limit`, `.max_consecutive_failures` (Workflow #384 D2-S4) | integer `0..=4294967295` | `96`, `3` |
| `scheduling.fact_refresh.recognition_join_timeout_ms` (Workflow #384 D2-S2) | milliseconds `0..=1800000` | `10000` |
| `scheduling.fact_refresh.overdue_ms` (Workflow #384 D2-S5) | milliseconds | none (unset: not reported) |
| `scheduling.suspension.lift_check_min_interval_ms` (Workflow #336 PKG-S3) | milliseconds | `60000` |
| `scheduling.pack_admission.check_in_check_config`, `.check_after_start` (Workflow #386 SR-3) | boolean | `true`, `true` |
| `scheduling.pack_admission.suspend_codes` (Workflow #386 SR-3) | list of codes, as `rerun.condition_codes` | `["recognition_sample_budget_invalid"]` |

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

## The `ledger_observer` section (Workflow #383 AO-1)

[ledger-observer.md](ledger-observer.md) §15 and [v012-register.md](v012-register.md) §5: every
key is parsed, checked and defaulted from v0.12.0 on, default `file`. The Runtime reads
`write_attempts`, `write_retry_backoff_ms`, `flush_attempt_stall_ms`, `health_probe_interval_ms`
and `dump_max_bytes` when the ledger opens: they make the writer's failure plan
([ledger-observer.md §9](ledger-observer.md#9-write-failure-retry-dump-fatal-ao-1)); a host
started without actingd's manifest reads the defaults file's values. The other keys take effect
with AO-2 (the capture queue, its batches and the request ring), AO-5 (the tag rules) and AO-8
(the probes and status).

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `ledger_observer.buffer_bytes` | integer, at least 1 | `67108864` (64 MiB) | no |
| `ledger_observer.buffer_records` | integer, at least 1 | `20480` | no |
| `ledger_observer.buffer.tier1_ratio`, `.tier2_ratio`, `.tier3_ratio` | decimal | `0.6`, `0.75`, `0.9` | no |
| `ledger_observer.buffer.hysteresis_ratio` | decimal | `0.1` | no |
| `ledger_observer.spill_batch_bytes` | integer, at least 1 | `4194304` (4 MiB) | no |
| `ledger_observer.dump_max_bytes` | integer, at least 1 | `1073741824` (1 GiB) | no |
| `ledger_observer.flush_batch_records`, `.flush_batch_records_busy` | integer, at least 1 | `256`, `2048` | no |
| `ledger_observer.write_attempts` | integer `1..=4294967295` | `5` | no |
| `ledger_observer.write_retry_backoff_ms` | list of milliseconds, one per attempt | `[0, 100, 250, 500, 1000]` | no |
| `ledger_observer.flush_attempt_stall_ms` | milliseconds, at least 1 | `120000` | no |
| `ledger_observer.flush_lag_warn_ms` | milliseconds, at least 1 | `5000` | yes |
| `ledger_observer.health_probe_interval_ms` | milliseconds, at least 1 | `1000` | no |
| `ledger_observer.probe_interval_ms` | milliseconds, at least 1 | `60000` | yes |
| `ledger_observer.probe_sample_keys` | integer, at least 1 | `1024` | yes |
| `ledger_observer.request_index_records` | integer, at least 1 | `4096` | no |
| `ledger_observer.status_gap_window_ms` | milliseconds, at least 1 | `86400000` | yes |
| `ledger_observer.tag.workbench_patterns` | list of key patterns | ledger-observer.md §15 | no |
| `ledger_observer.tag.reobserve_prefixes` | list of key prefixes | ledger-observer.md §15 | no |

A key that is not settable is read when the ledger opens, before any instruction value is
restored ("used before the ledger opens"): the capture queue and the writer with its probe
exist from then on. The section is checked whole at start, in `check-config` and after every
instruction; a broken rule is `config_value_invalid` (`section_check`) for the key it concerns:
the tiers rise strictly within (0, 1] (`tier1_ratio` above 0 and below `tier2_ratio`,
`tier2_ratio` below `tier3_ratio`, `tier3_ratio` at most 1); `hysteresis_ratio` lies within
(0, 1); (`tier3_ratio` − `tier2_ratio`) × `buffer_bytes` is at least `spill_batch_bytes`;
`write_retry_backoff_ms` lists exactly `write_attempts` waits; the tag lists hold non-empty texts
without spaces or control characters. A ratio or a wait of the wrong type is `wrong_type`.

## The `device_link` section (Workflow #398 RD-2)

Model-398 v1.1 §4.6 and [v012-register.md](v012-register.md) §1.52: how the Runtime watches the
wired link of a bound handset (one `adb track-devices` process per Runtime, only while a bound
handset is configured) and restores it. Every key is read where it is used, so an instruction may
change each while the Runtime runs: the watch's keys at its next restart, the others at the next
link change, restore or timer. The values are starting points the real-device check may tune.

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `device_link.watch.restart_backoff_ms` | list of milliseconds, the last one reused for every later restart | `[1000, 5000, 30000]` | yes |
| `device_link.watch.restart_limit` | integer `1..=100` | `5` | yes |
| `device_link.watch.restart_window_ms` | milliseconds, at least 1 | `600000` | yes |
| `device_link.settle_ms` | milliseconds `0..=60000` | `3000` | yes |
| `device_link.offline_action` | `wait` or `kick_once` | `wait` | yes |
| `device_link.restore_retry_ms` | list of milliseconds, one per new preparation after a failed one | `[5000, 15000, 60000]` | yes |

- A watch that ends is started again after its restart wait (`device_watch_restarted`); more
  restarts than `watch.restart_limit` within `watch.restart_window_ms`, a watch that cannot
  start, or an output of unknown shape is `device_watch_failed`: every handset is `unwatched`
  and unavailable until a person's self-check of a handset starts the watch again.
- A handset back at `device` for `settle_ms` gets its preparation on a self-check claim; a failed
  one is tried again after each wait of `restore_retry_ms`, and when they are used up the Runtime
  waits for the next link change or a person's self-check.
- `kick_once` sends one `adb -s <serial> reconnect` per `offline` episode (only that transport);
  `wait` sends nothing. No value makes the Runtime run `kill-server`, `reboot`, `usb` or `tcpip`.

The section is checked whole at start, in `check-config` and after every instruction; a broken
rule is `config_value_invalid` (`section_check`) for the key it concerns: both lists are
non-empty and `watch.restart_window_ms` is at least the first restart wait. A wait that is no
number of milliseconds is `wrong_type`.

## The `device_state` section (Workflow #398 RD-5b)

Model-398 v1.1 §7.8 and [v012-register.md](v012-register.md) §1.59: how the Runtime reads the
device state of a bound handset and when that state holds it. Every key is read where it is used,
so an instruction may change each while the Runtime runs: the sampling keys at the next sample,
the others at the next start gate or input. The values are starting points the real-device check
may tune; `in_use_gate` is off because the handset is a dedicated one (Alice, 2026-10-13,
model-398 §16 item 2).

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `device_state.sample_interval_ms` | milliseconds `5000..=3600000` | `60000` | yes |
| `device_state.fact_ttl_ms` | milliseconds, at least the sampling interval | `180000` | yes |
| `device_state.wake_settle_ms` | milliseconds `100..=10000` | `1000` | yes |
| `device_state.keepalive_margin_ms` | milliseconds | `60000` | yes |
| `device_state.keyguard_insecure` | `dismiss` or `wait` | `dismiss` | yes |
| `device_state.battery_min_percent` | integer `0..=100` | `15` | yes |
| `device_state.thermal_hold_at` | a thermal status (`none`, `light`, `moderate`, `severe`, `critical`, `emergency`, `shutdown`) | `severe` | yes |
| `device_state.thermal_resume_below` | a thermal status below `thermal_hold_at` | `moderate` | yes |
| `device_state.in_use_gate` | boolean | `false` | yes |
| `device_state.in_use_backoff_ms` | milliseconds `10000..=3600000` | `300000` | yes |

- A bound handset whose link is `device` is read every `sample_interval_ms` (read-only commands
  only; a sample never wakes the screen) and its `device.*` facts live for `fact_ttl_ms`.
- At a run's start, and before an input of a run once the time since the last one reaches the
  screen-off timeout read on the handset less `keepalive_margin_ms`, a dark screen gets one wake
  key and waits `wake_settle_ms`; a keyguard without a credential gets one dismissal under
  `dismiss`, or waits for a person under `wait`.
- On no power source below `battery_min_percent`, at or above `thermal_hold_at`, and (with
  `in_use_gate`) another application in front at a run's start, the handset holds; a thermal hold
  lifts only below `thermal_resume_below`, an in-use hold only at a sample after
  `in_use_backoff_ms`. No value makes the Runtime change a setting of the handset, press its
  power key or enter a credential.

The section is checked whole at start, in `check-config` and after every instruction; a broken
rule is `config_value_invalid` (`section_check`) for the key it concerns: `fact_ttl_ms` is at
least `sample_interval_ms` and `thermal_resume_below` is below `thermal_hold_at`.

## The `startup_discovery` section (Workflow #390 DISC-1)

Coordinator ruling 2 of wave 18 and [v012-register.md](v012-register.md) §1.89: how often a start
tries the instance discovery again while it answers `instance_discovery_unavailable`, as after a
boot where the Runtime starts before the emulator ([provider-startup.md](provider-startup.md),
"Discovery unavailable at start"). The host reads the section once, from the manifest (the
defaults file's values for a host started without one), and hands it to the provider startup,
which runs before any instruction value is restored; so no key is settable, and the file changes
them at the next start.

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `startup_discovery.attempts` | integer `1..=100`, the most attempts of one start, the first included (`1` tries once) | `25` | no (read once at assembly) |
| `startup_discovery.retry_interval_ms` | milliseconds `1000..=300000`, the wait after each failed attempt but the last, on the Runtime clock | `10000` | no (read once at assembly) |

The defaults wait 24 times 10 s, about four minutes, which covers a normal cold start of the
emulator (about three minutes) with room to spare; each attempt itself takes at most
MuMuManager's 10 s timeout. A value outside its range refuses a start and `check-config` as
`config_value_invalid` (`out_of_range`, with `range_min` and `range_max`) for its key.

## Facts

| Fact (scope `runtime`) | Rows | Recorded |
|---|---|---|
| `config.subsystems` | unchanged; Workflow #384 D2-S2 adds the row `recognition_packages` when the configuration binds a recognition package: enabled when the start kept one, with one `note` link of `fact_reader_binding_ignored` per binding it runs without | at start |
| `config.selection` | one: `config_sha256`, `defaults_sha256` (`sha256:<hex>`), `config_generation` (decimal text, installed Runtime only) | at start |
| `config.catalog` (Workflow #389 MCP-9a, register A5) | one, the active policy catalog generation: `catalog_id`, `catalog_version` (decimal text), `catalog_hash`, `activated_event_id` (the event id of the `catalog.activated` or `catalog.rolled_back` record that made it active), and when known `approval_id` and `approval_disposition` (the latest approval decision whose target is that generation), `previous_catalog_id` and `previous_catalog_version` (decimal text; the generation that record replaced); no fact while no generation is active | at start, after each committed catalog transition and after each approval decision, each time only when it changed |
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
