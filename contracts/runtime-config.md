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
`startup_discovery` section below (default `file`, not settable, read once at assembly).
Workflow #406 T12's rows: the `touch` section below (default `file`, settable). PERF-B1's rows:
the `execution` section below (default `file`, settable). Workflow #392 DA-4b-D's rows: the
`display_safe_area` section below (default `file`, settable) and the per-instance
`display_safe_area.model` (string, default `section:display_safe_area.model`, settable). RB-2's
rows: the `host` section below (string `normal` | `low`, default `file`, not settable: applied
before the ledger opens; since 0.12.28). A manifest key the table does not have fails
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
| `recovery_ladder.fence_recheck.interval_ms` | milliseconds 10000..=86400000 (#411 A38) | `300000` |
| `recovery_ladder.fence_recheck.max_attempts` | integer 0..=1000 (#411 A38); `0` turns the recheck off | `12` |
| `recovery_ladder.busy.poll_ms` | milliseconds 100..=`busy.outcome.recheck_ms` (#411 B37) | `500` |
| `recovery_ladder.busy.outcome.recheck_ms` | milliseconds; plus `hold.margin_ms` within the longest renewal (#411 B37) | `60000` |
| `recovery_ladder.busy.outcome.max_wait_ms` | milliseconds 0..=86400000 (#411 B37); `0` holds nothing | `3600000` |
| `recovery_ladder.busy.outcome.on_exhausted` | `stop` \| `escalate` (#411 B37) | `stop` |
| `recovery_ladder.busy.screen.classes` | list of ladder class tokens, each once (#411 A35); `[]` turns the settle gate off | `["pack_logic_mismatch"]` |
| `recovery_ladder.busy.screen.probe_frames` | integer 2..=5 (#411 A35) | `5` |
| `recovery_ladder.busy.screen.probe_interval_ms` | milliseconds 100..=10000; `(probe_frames - 1)` intervals plus `hold.margin_ms` within the longest renewal (#411 A35) | `1000` |
| `recovery_ladder.busy.screen.change_max_milli` | unsigned 32-bit integer, the frame-change threshold (#411 A35; [color-digest.md](color-digest.md), "Frame change") | `1500` |
| `recovery_ladder.busy.screen.recheck_ms` | milliseconds, at least `busy.poll_ms`; plus `hold.margin_ms` within the longest renewal (#411 A35) | `15000` |
| `recovery_ladder.busy.screen.max_wait_ms` | milliseconds 0..=86400000 (#411 A35); `0` probes only | `900000` |
| `recovery_ladder.busy.screen.on_exhausted` | `escalate` \| `stop` (#411 A35) | `escalate` |

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

#411 A38 (`actingd-config` revision 8): `fence_recheck` is the periodic self-check of a fenced
instance that waits for a person ([emulator-control.md](emulator-control.md), "Resource fence"):
the first one `interval_ms` after the wait began, each next one `interval_ms` after the failed one
before it ended, at most `max_attempts` per wait. The lease sweep reads both at each tick, so a
change through an instruction (`ac_config_set`) applies from the next tick; `Status` reports
`max_attempts` in effect. A value outside its range is `out_of_range` (with `range_min` and
`range_max`). The defaults (every five minutes, at most twelve times, one hour of rechecks) let a
device that came back by itself, such as a capture channel that could not open for a moment, run
again within minutes, without a self-check every few seconds while a device is really away; after
the last one a person is needed, as before.

#411 B37 (slice A35-S1; `actingd-config` revision 8, extended; coordinator decision Q3 of
2026-10-21): `busy` is the ladder's hold after a rung run that ended on a mapping its package
marks `busy` ([emulator-control.md](emulator-control.md), "Busy ends"). After such an end the
ladder holds its key for `outcome.recheck_ms` and runs the return-home rung again, as long as the
climb has held less than `outcome.max_wait_ms` since its first busy end (each hold no longer than
what is left); at that limit `outcome.on_exhausted` either ends the ladder without a restart
(`stop`, `recovery_ladder_hold_exhausted`) or goes on to the next rung (`escalate`). While it
holds, the ladder makes its holding check every `poll_ms`. The check adds: `poll_ms` from 100 to
`outcome.recheck_ms` and `outcome.max_wait_ms` within a day (`bound_out_of_range`), and
`outcome.recheck_ms` plus `hold.margin_ms` within the longest renewal, the renewal a hold makes
before it waits (`renewal_cap_exceeded`); `on_exhausted` is one of its two words (`out_of_range`,
like the section's other rules). All four keys are settable and part of the section's digest; a
ladder holds by the values it was admitted with. The defaults (recheck every minute, at most one
hour, then stop) bring an instance home within a minute of an activity's end and never restart
the game while its package says it is busy; they are recalibrated after the real-device check.

#411 A35 (slice A35-S2; `actingd-config` revision 9; design `a35-design.md` §3 with decision Q8
of 2026-10-21): `busy.screen` is the settle gate after a return-home failure
([emulator-control.md](emulator-control.md), "Settle gate"). After a return-home failure whose
ladder class `screen.classes` lists, the ladder takes `screen.probe_frames` read-only frames
`screen.probe_interval_ms` apart; two consecutive frames are unchanged when the Runtime's one
frame-change measure between them ([color-digest.md](color-digest.md), "Frame change") is at
most `screen.change_max_milli` (default and range those of the measure's threshold: 1500, an
unsigned 32-bit integer). While the screen changes the ladder holds `screen.recheck_ms` and
probes again, within `screen.max_wait_ms` of its first changing verdict; at that limit
`screen.on_exhausted` goes on to the next rung (`escalate`) or ends the ladder without a restart
(`stop`, `recovery_ladder_hold_exhausted`). The check adds: a class that is not a ladder class
token (`code_unknown`; another JSON value `wrong_type`) or a class named twice (`duplicate_id`);
`probe_frames` from 2 to 5, `probe_interval_ms` from 100 to 10000, `change_max_milli` from 0 to
4294967295 and `max_wait_ms` within a day (`bound_out_of_range`); `busy.poll_ms` above
`screen.recheck_ms` (`bound_out_of_range` on `busy.poll_ms`, whose range ends at the shorter of
the two rechecks); `screen.recheck_ms` plus `hold.margin_ms`, and `probe_frames - 1` intervals
plus `hold.margin_ms`, within the longest renewal, the renewals the ladder makes before it waits
and before it probes (`renewal_cap_exceeded`); `on_exhausted` is one of its two words
(`out_of_range`). All seven keys are settable and part of the section's digest; a ladder holds
by the values it was admitted with. The defaults (five frames a second apart, a recheck every 15
seconds, at most 15 minutes, then escalate) are recalibrated after the real-device check.

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
| `host_claims.run_end_grace_ms` | milliseconds, `10000..=600000`, settable (read when a run reaches its deadline) | `120000` |

Before a step a Runtime-held key renews to the step's bound (capped at a task's longest deadline
plus the heartbeat bound, as every claim TTL), unless its remaining time plus
`renewal_slack_ms` already covers that bound. The value is read at each renewal; a value that is
not milliseconds is refused (`config_value_invalid`, `wrong_type`).

#411 A64 (ruling C-43, `actingd-config` revision 11): a contained run that reaches its deadline
while it still holds its lease enters its end phase. Before any of its end work (the diagnostic
seal, the capture summary, the terminal, the close) its lease is renewed once to
`run_end_grace_ms` from then, and every step of its end runs under that lease. The lease sweep
leaves the lease of a run inside its end grace alone, also when that renewal was refused; after
the grace it may expire it as any lapsed lease. A close in the end phase that meets the lapsed
lease fences only that instance (Workflow #403; the refusal is recorded once per run), and once
the lapsed lease has ended the instance's session, which the multi-Nemu rule closes at every lease
end, is closed under a dedicated resource-close lease (left to a holder that took the key on). A
terminal that finds its lease already expired by the sweep is still recorded, its outcome
carrying `lease_expired`, and fences the instance; neither stops the Runtime. A run whose lease
ended before its end began enters no end phase, and every other lease (an ordinary client lease
included) keeps its behaviour from before C-43.

ISO-5a (Workflow #419, ISO-M rows A08, A11; C-43 point 3, since Runtime 0.12.27; no new key, no
revision): a contained run that ends otherwise (a success, another failure, a cancel) enters its
end phase the same way while it holds its lease and the lease has not lapsed yet; a run whose
failure poisons (a ledger failure) ends as before. Its lease is renewed once to
`run_end_grace_ms` from then only when it would lapse inside that grace; a lease that outlasts the
grace is kept as it is. The key is therefore read when any run's end begins. A scheduled run
whose success terminal was committed and whose lease end then met its lease already ended (its end
ran past the grace) settles as a success; only its receipt fails, never the Runtime.

## The `standard_packs` section (Workflow #373 NAV-6; moved by D1c)

| Key | Type | Default |
|---|---|---|
| `standard_packs.root` | string (a path), not settable (read once at assembly) | none: without it no standard-pack root is read |
| `standard_packs.check_interval_ms` | milliseconds, `1000..=3600000`, settable (ROUT-L RL-1b, since 0.12.13) | `60000` |
| `standard_packs.cached_generations` | integer, `0..=8`, settable (ROUT-L RL-1b, since 0.12.13) | `1` |

The directory that holds one root per game (`<root>/<game>/`), written once by the installer. A
relative path resolves against the configuration file's directory; an empty one is refused.
D1c moves the section's structure from actingd's own configuration into this crate (NAV-6
deviation 5); the file member and its meaning are unchanged, and `config.parameters.standard_packs`
reports it as the file writes it when the file names it. Changing it needs a commit and a
restart.

ROUT-L RL-1b (frozen model v1.0 §3.3, §3.8; [standard-pack-schedule.md](standard-pack-schedule.md),
"The scheduler workbench"): when the `policy` section names no `catalog` (composer mode), the
scheduler workbench composes the rules from the `schedule.json` of the games in use, and
`standard_packs.root` is required (`config_value_invalid`, `config_key` `standard_packs.root`).
`check_interval_ms` is the least time between two checks of the games in use, at the start of a
policy cycle; `cached_generations` is how many composed generations older than the active one
stay in memory (it replaces the fixed bound of 8). Both are read at each check and can change
while the Runtime runs. A configuration that names `policy.catalog` keeps the configured catalog
path as before; the workbench then reads nothing.

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
| `performance.pacing.*` (`src/performance/pacing.rs`) | PERF §10.3, 26 keys: `mode` (`observe` \| `pace` \| `full`, default `observe` in every release), `interpolate`, `curve` (a list of at most 70 points), the factor cap, hysteresis and sustains, pause timing, waits and gaps, `pressure_floor.*`, `tax_exempt_from_stage`, `heavy_dispatch_limit.normal` / `.paused`, `clock_jump_ms`, `hold.*`, `pause_probe.*`; and Workflow #427-B's optional `max_concurrent_runs` (an integer `1..=65535`, **no default**: a configuration that does not name it has no such limit and no `config.parameters` row; since 0.12.22) | `heavy_dispatch_limit.*`, `max_concurrent_runs` and `clock_jump_ms`: the performance controller, handed over by the start-up assembly (P-c; before, code constants; `max_concurrent_runs` from #427-B, which the policy host hands to every evaluation: at most that many policy dispatches admitted or running across every instance, those chosen earlier in the round included; `1` runs one at a time; [scheduling/README.md](scheduling/README.md), "Shared Run Limits"); `pre_input_wait_base_ms`, `curve[].extra_retries` and `pressure_floor.extra_retries`: P-k (the pre-input wait and the extra wait rounds each step is handed, Workflow #385 P-k S5); the rest: P-m, P-k, P-s, P-h |
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
`scheduling.recognition.sample_timeout_retries` and the instance's pacing factor in effect
(`performance.pacing.mode`, Workflow #385 P-k; see
[performance-control.md](performance-control.md), "Step pacing") at each step of a contained
run (the kernel's `step_pacing()`), the others take effect with the 0.12.x slices that read
them. Their meaning is
in [policy-suspension.md](policy-suspension.md) (`recognition`, `rerun`),
[cost-commit.md](cost-commit.md) (`cost_commit`) and [scheduling-blackout.md](scheduling-blackout.md)
(`blackout`); `feature_lock.*` (how long a recorded locked feature stays valid, and the codes
that end a run on one) is read by slices SR-5a and SR-5b, `not_run.diagnose_rows` (the most rows
`ac_diagnose` lists under `not_run` per instance) by MCP-19b (Workflow #386 SR-6b: actingctl and
MCP read it from the program fact `config.parameters.scheduling` at every diagnosis). Workflow #384 D2-S2 reads
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
| `scheduling.rerun.condition_codes` | list of codes the catalog lists and has not retired | the model's eight codes and, since 0.12.28 (Alice's decision 92, 2026-10-29), `contained_task_paused` ([policy-suspension.md](policy-suspension.md), "Which runs") |
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
| `scheduling.blackout.quiet_horizon` (Workflow #386 SR-4a1) | `deadline` \| `expected` | `deadline` |
| `scheduling.day_boundary.quiet` (Workflow #386 SR-4a1) | boolean | `true` |
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
| `scheduling.fact_refresh.expected_duration_ms` (SCH-M ED-1) | milliseconds `1..=1800000` | `60000` |
| `scheduling.expected_runtime.enabled` (SCH-M ED-1) | boolean | `true` |
| `scheduling.expected_runtime.runs` (SCH-M ED-1) | integer `1..=16` | `5` |
| `scheduling.expected_runtime.margin_ms` (SCH-M ED-1) | milliseconds `0..=3600000` | `300000` |
| `scheduling.suspension.lift_check_min_interval_ms` (Workflow #336 PKG-S3) | milliseconds | `60000` |
| `scheduling.pack_admission.check_in_check_config`, `.check_after_start` (Workflow #386 SR-3) | boolean | `true`, `true` |
| `scheduling.pack_admission.suspend_codes` (Workflow #386 SR-3) | list of codes, as `rerun.condition_codes` | `["recognition_sample_budget_invalid"]` |
| `scheduling.remedy.enabled` (Workflow #411 B15) | boolean | `true` |
| `scheduling.remedy.max_per_round` (Workflow #411 B15) | integer `1..=8` | `2` |
| `scheduling.remedy.choices` (Workflow #411 B15) | list of remedy choice rows (objects) | `[]` |
| `scheduling.parallel.enabled` (Workflow #427-A) | boolean | `false` |
| `scheduling.evaluation.retry_ms` (ISO-PE, #419) | milliseconds `1..=3600000` | `15000` |
| `scheduling.evaluation.escalate_after` (ISO-PE, #419) | integer `1..=4294967295` | `3` |

The section is checked whole at start, in `check-config` and after every instruction: a code
list entry the catalog does not list is `config_value_invalid` with `code_unknown`, a retired
one `code_retired`; a blackout entry (of the host list or, Workflow #386 SR-4a1, of an
instance's own list) that breaks a rule of [scheduling-blackout.md](scheduling-blackout.md)
("Configuration blackouts": `weekly` and `once` entries, at most 64 per list) refuses a start and
`check-config` with `blackout_config_invalid` (`field`, `blackout_source` naming the list, and
`blackout_id` when the entry has a usable id: one to 256 bytes, no control character) and an
instruction with `config_value_invalid` (`section_check`);
`random_delay.min_ms` above `random_delay.max_ms` is `config_value_invalid` (`section_check`) for
`min_ms`.

**Per instance (Workflow #405).** `instance.<instance id>.scheduling.random_delay.enabled`,
`.min_ms` and `.max_ms`, in the file the entry's `scheduling.random_delay` member, settable,
default `section:scheduling.random_delay.<member>`. The pair in effect (each side the
instance's own or the section's) is checked: `min_ms` above `max_ms` is `config_value_invalid`
(`section_check`) for the instance's `min_ms` key, so a change of one side alone is checked
against the other. The scheduler reads them from #405 D1 on: at every policy cycle it reads, for
each instance of the snapshot, the pair in effect (each member the instance's own key, otherwise
the section's), and an instance whose `enabled` is true in effect delays every task the policy
dispatches to it ([scheduling-blackout.md](scheduling-blackout.md), "Order of the deferrals",
position 9). Every member is settable while the Runtime runs; a change applies to the draws made
after it (a draw already made keeps its delay), and `enabled = false` releases the instance's
waiting tasks at the next evaluation.

**Blackouts and the day boundary per instance (Workflow #386 SR-4a1, SCH-M §3.2 D-1, D-6;
`actingd-config` revision 11).** `instance.<instance id>.scheduling.blackout`, in the file the
entry's `scheduling.blackout` member, is the instance's own blackout list: no default (an instance
whose entry names none has no entries of its own and reports no row), settable once the entry
names one, its entries checked as the host list's are.
`instance.<instance id>.scheduling.day_boundary.quiet`, in the file the entry's
`scheduling.day_boundary.quiet`, settable, default `section:scheduling.day_boundary.quiet`. Accessors `EffectiveConfig::instance_blackouts` and
`EffectiveConfig::day_boundary_quiet`; SR-4a2 reads them, and `scheduling.blackout.quiet_horizon`,
at every policy cycle.

**Expected duration (SCH-M slice ED-1, frozen model v1.0 §3.4; `actingd-config` revision
11).** `scheduling.expected_runtime.enabled`, `.runs` and `.margin_ms` set the expected duration E
of a task package on an instance: the mean of its latest `runs` successful scheduled runs, rounded
up, plus `margin_ms`, at most the task's `loop_budget.max_runtime_ms`; with no run, or with
`enabled` false, the declared value ([scheduling/README.md](scheduling/README.md), "Settlement
Feedback And Eligibility Age"). Each is also per instance,
`instance.<instance id>.scheduling.expected_runtime.<member>`, in the file the entry's
`scheduling.expected_runtime` member, settable, default `section:scheduling.expected_runtime.<member>`;
`EffectiveConfig::expected_runtime(instance_id)` returns each member's value in effect, the
instance's own or the section's. `scheduling.fact_refresh.expected_duration_ms` is the declared
duration D of a fact refresh read. ED-1 parses and checks the keys; slice ED-2 reads them: each
instance's `enabled`, `runs` and `margin_ms` at every write of a run-duration sample and every
projection of the evaluation inputs ([instance-fact-store.md](instance-fact-store.md), "Run duration
facts"), and `scheduling.fact_refresh.expected_duration_ms` at every evaluation and admission of a
fact refresh read, whose intent record carries a value other than the spec's nominal 60 000, so
recovery reserves it again. ED-2 adds no key.

**Per-instance lanes (Workflow #427-A; `actingd-config` revision 19).**
`scheduling.parallel.enabled` (default `false`, settable) chooses where the policy driver runs
an admitted scheduled run: `false`, on its own thread, one run after another, as before;
`true`, on the run's instance worker (`actingd-instance-<alias>`), so a long run on one instance
holds neither the round nor another instance ([scheduling/README.md](scheduling/README.md),
"Per-Instance Lanes"). actingd reads it at the start of every policy round, the initial round
included, so an instruction (`ac_config_set`, `actingctl config set`) takes effect at the next
round; a run already handed to a worker finishes and is settled there. Accessor
`EffectiveConfig::scheduling_parallel_enabled`.

**Skipped evaluations (ISO-PE, #419; `actingd-config` revision 20, which RB-2 raised in the same
release).** A policy evaluation whose failure is not global is skipped and recorded
(`policy_evaluation_skipped`), and evaluated again `scheduling.evaluation.retry_ms` later
(default `15000`, `1..=3600000`: never zero, so a failure that repeats is no busy loop);
`scheduling.evaluation.escalate_after` skips in a row (default `3`, at least `1`) record
`policy_evaluation_awaiting_person` once, and the Runtime keeps running
([scheduling/README.md](scheduling/README.md), "Evaluation failures"). Both are settable; the
host reads them at every skipped evaluation, so an instruction takes effect at the next one. In
the file they are the `scheduling` section's `evaluation` member; a Runtime before 0.12.28
refuses a file that names it, because the section denies unknown members. Read with
`EffectiveConfig::scheduling` (`SchedulingConfig::evaluation`).

**Remedies (Workflow #411 B15, [remedies.md](remedies.md); parsed and checked by slice R3,
v0.12.3).** `scheduling.remedy.enabled` turns declared remedies on (`false`: every remedy key is a
plain named outcome again, the package checks still run); `scheduling.remedy.max_per_round` is the
number of remedy requests per blocked layer and round (`1..=8`); `scheduling.remedy.choices` is the
host-wide list of remedy choice rows, objects of the shape of the instance fact
`session.remedy.choices` (`{"row": "allow", "need", "capability"}`, `{"row": "none", "need"}`,
`{"row": "parameter", "capability", "parameter", "value"}`; tokens `^[a-z][a-z0-9_]{0,63}$`,
`none` never a capability, no duplicate row, no `none` with an `allow` of the same need, no two
values for one parameter of a capability, at most 256 rows). Per instance,
`instance.<instance id>.scheduling.remedy.choices` (in the file the entry's
`scheduling.remedy.choices` member), default `section:scheduling.remedy.choices`: when
set, it replaces the section's list as a whole. All four are settable; their defaults live only
in `actingd.defaults.json`, never as code constants. A row that breaks the shape refuses the
configuration as `config_value_invalid` (`config_key`) at load, in `check-config` and in
`ConfigureSettings`, with `config_value_problem` `wrong_type` (a row that is no object; a `row` or
field missing or not a string; a member its kind does not have), `out_of_range` (an unknown `row`;
a name outside the charset; `none` as a capability; more than 256 rows) or `section_check`, at the
later row (a row twice; a `none` with an `allow` of the same need; two `parameter` rows of the same
capability and parameter with different values); a row whose names the bound packages do not
declare is not refused here but is a check-config note `policy_remedy_declaration_unusable` and, at use, a stop for a person
(slices R4 and R7). The section rows are recorded in `config.parameters.scheduling`, the
per-instance rows in `config.parameters.instance`. The four keys raise `actingd-config` to
revision 6 in the release that carries them (once per release, whichever slice of the release
needs it first): v0.12.3 declares [5, 6]. The chain depth (3) is no key.

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
| `ledger_observer.read_budget_ms` (#411 A51) | milliseconds `1..=9999` | `1000` | no |
| `ledger_observer.close_timeout_ms` (#411 A51) | milliseconds, above `read_budget_ms` | `60000` | no |
| `ledger_observer.open_timeout_ms` (P0-LO) | milliseconds `10000..=600000` | `600000` | no |
| `ledger_observer.open_warn_ratio_milli` (P0-LO) | integer `100..=1000` (thousandths) | `500` | no |
| `ledger_observer.history_page_rows` (#422 LN-5) | integer `1..=4096` | `256` | no |
| `ledger_observer.writer_reply_timeout_ms` (#422 LN-TUNE) | milliseconds `1000..=600000`, above `read_budget_ms` | `10000` | no |
| `ledger_observer.open_verify_tail_records` (#422 LN-8) | integer `1..=1000000` | `4096` | no |
| `ledger_observer.scrub_rows_per_second` (#422 LN-9) | integer `1..=100000` | `625` | yes |
| `ledger_observer.scrub_chunk_rows` (#422 LN-9) | integer `1..=4096` | `256` | no |
| `ledger_observer.scrub_pass_interval_ms` (#422 LN-9) | milliseconds `60000..=86400000` | `2400000` | no |
| `ledger_observer.read_check_vm_steps` (#422 LN-WS) | integer `100..=1000000` | `10000` | no |
| `ledger_observer.history_scan_rows` (#422 LN-WS) | integer `1024..=1048576` | `65536` | no |
| `ledger_observer.frame_query_steps_per_second` (#422 LN-7c Q) | integer `1000..=100000000` | `500000` | yes |
| `ledger_observer.frame_query_chunk_frames` (#422 LN-7c Q) | integer `1..=4096` | `256` | no |
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
without spaces or control characters; `read_budget_ms` lies below the writer's reply limit
`writer_reply_timeout_ms` (#411 A51; #422 LN-TUNE) and `close_timeout_ms` above `read_budget_ms`
(#411 A51). A ratio or a wait of the wrong type is `wrong_type`.

P0-LO (Runtime v0.12.11, `actingd-config` revision 10): `open_timeout_ms` is the deadline of the
start's ledger open (it was a fixed 120 s): the host's own open takes it as its maintenance
deadline, in whole seconds rounded up, and so does the frame retention recovery right after the
open; the offline `ledger-maintenance` and `unlock-owner` keep their own 120 s.
`ledger_opened` and `ledger_open_measured` report it as `timeout_ms`. When the open's measured
time (the sum of the phases `ledger_opened` reports) exceeds `open_timeout_ms` ×
`open_warn_ratio_milli` / 1000, actingd writes one warning `ledger_open_slow` (`elapsed_ms`,
`timeout_ms`, `event_count`) right after `ledger_opened`
([ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer)). Both keys
are read when the ledger opens (not settable); a value outside its range is `config_value_invalid`
(`out_of_range`).

#422 LN-5 (v0.12.13, `actingd-config` revision 12): `history_page_rows` is the rows of one page of a
history read; the ledger reads its history from SQLite a page at a time, and the Runtime's paged
history reads take as many events per page, at most a query page's 1 024
([ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer)). It is read
when the ledger opens (not settable); a value outside `1..=4096` is `config_value_invalid`
(`out_of_range`).

#422 LN-TUNE (`actingd-config` revision 12): `writer_reply_timeout_ms` is how long a caller waits
for one reply of the ledger writer (until LN-TUNE the ledger's fixed 10 s): every writer command,
a subscription's replay page, the close's confirmation of deferred appends and the host's
boundaries that confirm them wait it; one that runs out is `writer_response_timeout`
([ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer)). It is read
when the ledger opens (not settable); a value outside `1000..=600000` is `config_value_invalid`
(`out_of_range`), and a `read_budget_ms` not below it is `config_value_invalid`
(`section_check`) for `read_budget_ms`.

#422 LN-8 (`actingd-config` revision 13): `open_verify_tail_records` is how many of the newest
records the start's ledger open reads and authenticates; the open reads no older record, which a
reader authenticates when it reads it and `ledger-maintenance verify` and the background re-check
read in full ([ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer);
[ledger-store.md](ledger-store.md#open-ownership-and-append-durability)). It is read when the
ledger opens (not settable); a value outside `1..=1000000` is `config_value_invalid`
(`out_of_range`).

#422 LN-9 (`actingd-config` revision 13; LEAN-L §6, Alice's ruling 57): the pace and the limits
of the ledger's background scrub of the whole chain
([ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer)).
`scrub_rows_per_second` is the rows a second it reads and authenticates; it is settable, and the
scrub reads the value in effect after each chunk (at start, once the instruction values are
restored, and after every instruction). `scrub_chunk_rows` is the rows of one chunk, each in a
read transaction of its own, and `scrub_pass_interval_ms` the least time from the start of one
pass to the start of the next; both are read when the ledger opens (not settable). The defaults
read 1.5 million records in about 40 minutes. A value outside its range is
`config_value_invalid` (`out_of_range`).

#422 LN-WS (`actingd-config` revision 14; the production stop of 2026-10-26 19:12:43):
`read_check_vm_steps` is how often, in SQLite virtual-machine steps, a read that serves a Runtime
request looks at its `read_budget_ms` inside a statement, and `history_scan_rows` how many
sequences one history statement of a view page spans at most
([ledger-observer.md §15](ledger-observer.md#15-configuration-keys-ledger_observer)). Both are
read when the ledger opens (not settable); a value outside its range is `config_value_invalid`
(`out_of_range`).

#422 LN-7c Q (`actingd-config` revision 16; `rd-1026/ln7c/final-query.md` §1.5, §5):
`frame_query_steps_per_second` is the SQLite virtual-machine steps a second the frame
cleaner's statements take; it is settable, and the cleaner reads the value in effect after
each statement. `frame_query_chunk_frames` is the frames of one statement L or K, read at
start (not settable). A value outside its range is `config_value_invalid` (`out_of_range`).
The top-level key `frame_retention_evidence_max_age_days` (integer `1..=3650`, default `15`,
settable; beside the three frame retention switches of revision 4) is the age, by a frame's
ingestion time, at which the cleaner removes an evidence frame in `kept\` that no agent or
person confirmed reading; a confirmed one goes at its next pass, whatever its age. The
cleaner takes it at each pass start, so an instruction takes effect at the next pass; a value
outside its range or of another type is `config_value_invalid`. actingd reports it in
`config.parameters.top_level`, which `actingledger frames` reads
([ledger-store.md](ledger-store.md), "Frame cleaner" and "Frame listing"). There is no 14-day
key (the 14-day question answered A).

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

## The `touch` section (Workflow #406)

Workflow #406 T12 (plan Q13; [v012-register.md](v012-register.md) §1.136;
[resource-declarations.md](resource-declarations.md), "Touch sampling"): how a box press's point
is drawn around the box's center and how long the Runtime remembers where its presses landed.
Host-wide. The host reads every key at each step of a run and at each choice of a landing point,
so an instruction may change each while the Runtime runs and takes effect at the next step.
`actingd-config` revision 7 ([component-interfaces.md](component-interfaces.md)) adds the
section; a file that does not name it keeps its revision.

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `touch.sampling.sigma_ratio_milli` | integer `1..=1000`, the spread of a centre-hotspot draw in thousandths of the box's width and of its height | `250` (the box's edge lies two spreads from its center) | yes |
| `touch.sampling.max_redraws` | integer `0..=64`, how often a draw outside the box is drawn again before the center is taken | `8` | yes |
| `touch.sampling.candidate_count` | integer `1..=16`, the candidates of a single-region press (`1`: one point, no least-recent choice) | `4` | yes |
| `touch.history.points_per_box` | integer `1..=256`, the most landing pixels one box remembers (the newest) | `16` | yes |
| `touch.history.max_boxes` | integer `1..=65536`, the most boxes one instance remembers (the box unused longest is forgotten) | `1024` | yes |

A value outside its range is `config_value_invalid` (`out_of_range`, with `range_min` and
`range_max`) for its key at start, in `check-config` and in `ConfigureSettings`; a value that is
no integer is `wrong_type`. The rows are those of `config.parameters.touch`. The landing history
itself is memory only and never recorded.

## The `emulator_control` section (#411 A50)

#411 A50 ([v012-register.md](v012-register.md) §1.158; [emulator-control.md](emulator-control.md),
"Restart"): how long a restart waits for its old VM to exit before it launches. A restart (an
operator's `emulator restart`, or the ladder's emulator-restart rung) whose Stop's readiness wait
expired reads the emulator status (as a self-check does) until it reads the old VM stopped, at
most this long; still running at the bound, the restart fails
`emulator_restart_previous_instance_running` and launches nothing. Host-wide. The host reads the
key at every such restart, so an instruction may change it while the Runtime runs.
`actingd-config` revision 8 ([component-interfaces.md](component-interfaces.md)) adds the section;
a file that does not name it keeps its revision.

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `emulator_control.restart_exit_wait_ms` | milliseconds `1000..=300000`, the longest wait for the old VM to exit, on the Runtime clock | `60000` | yes |
| `emulator_control.watch_enabled` | boolean: whether the emulator watch polls the emulator status of the discovery-bound instances (emu-watch R2) | `true` | yes |
| `emulator_control.watch_interval_ms` | milliseconds `5000..=3600000`, the time between two polls, on the Runtime clock | `60000` | yes |
| `emulator_control.restart_attempts` | integer `1..=100`, the most starts of one configured instance's stopped emulator by the watch within `restart_window_ms` (emu-watch R3) | `3` | yes |
| `emulator_control.restart_window_ms` | milliseconds `60000..=86400000`, the window `restart_attempts` counts in | `1800000` | yes |

A value outside its range is `config_value_invalid` (`out_of_range`, with `range_min` and
`range_max`) at start, in `check-config` and in `ConfigureSettings`; a value that is no integer is
`wrong_type` (`watch_enabled`: no boolean). The rows are those of
`config.parameters.emulator_control`. The client's receipt wait of a restart adds the range's
maximum of `restart_exit_wait_ms` (745 s).

emu-watch (Alice's ruling 35; [emulator-control.md](emulator-control.md), "Emulator watch"): the
watch reads its four keys at every tick, so an instruction changes them while the Runtime runs;
`watch_enabled` false stops the polls (and so the watch's restarts) until it is true again.
`actingd-config` revision 11 ([component-interfaces.md](component-interfaces.md)) adds the four
keys; a file that names none of them keeps its revision.

## The `watchdog` section (#411 A66)

#411 A66 ([runtime-watchdog.md](runtime-watchdog.md), "Start window"; coordinator ruling 10-24):
how long a start may take before the watchdog, or `actingctl start` and `restart` without
`--ready-timeout-s`, calls it `start_timeout`. The same window bounds which open watchdog start an
answering owner belongs to, and a waiter for the watchdog's `run.lock` waits it plus 20 s. actingd
parses and checks the section with the rest of the file (its row is that of
`config.parameters.watchdog`) but never uses it. The watchdog and `actingctl` read it from the
configuration file they already read for the state root, at every tick or command, through this
crate (`watchdog::start_window_ms`), never from the configuration in effect, so it is not settable.
`actingd-config` revision 11 ([component-interfaces.md](component-interfaces.md)) adds the section;
a file that does not name it keeps its revision.

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `watchdog.start_window_ms` | milliseconds `60000..=3600000`, the longest a start may take | `900000` | no (read from the file by the watchdog and `actingctl`) |

A value outside its range is `config_value_invalid` (`out_of_range`, with `range_min` and
`range_max`) at actingd's start, in `check-config`, and on every watchdog tick or `actingctl start`
or `restart` (the tick's `watchdog_misconfigured`); a value that is no integer is `wrong_type`.

## The `execution` section (PERF-B1, Workflow #407 B1)

PERF-B1 ([v012-register.md](v012-register.md) §1.177; [resource-declarations.md](resource-declarations.md),
"Capture cadence"): the keys the contained-task kernel takes from the configuration, so far its
capture cadence (`execution.capture.*`), how often a run captures while a step waits for a page; the
planned `execution.defaults.*` (register §5) would join the section. A package
declares its capture interval for every wait (`control.capture_interval_ms`, 50 ms when it declares
none) and for one step's wait (`expect_after.interval_ms`; a `linear_steps` transition's
`interval_ms`). The run refuses a package that declares an interval above
`capture.max_interval_ms` before its first capture, with the code admission gives a broken
interval (`contained_task_control_invalid`; `contained_task_linear_invalid`,
`reason=transition_interval`, for a transition); admission itself bounds an interval only by the
key's range maximum. While a step waits for a page (the page wait and the confirmation of the
page-graph path, the waits of a `linear_steps` step), each frame unchanged from the wait's frame
before it by the Runtime's frame-change measure (#411 B12, the whole frame) doubles the next pause,
up to `capture.idle_ceiling_ms` or the declared interval, whichever is longer, and the first changed frame
returns it to the declared interval; the pause is still cut at the wait's remaining time and the
task's deadline. Host-wide. The host hands the three keys to the kernel with each step's pacing, so
an instruction may change each while the Runtime runs: the ceiling takes effect at the next run, the
back-off at the next step. A runtime without the configuration (offline simulation, fixtures) uses
the default ceiling and no back-off. `actingd-config` revision 11
([component-interfaces.md](component-interfaces.md)) adds the section; a file that does not name it
keeps its revision.

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `execution.capture.max_interval_ms` | milliseconds `5000..=1800000`, the largest capture interval a package may declare (at least the bound every package was held to before PERF-B1; at most the longest wait a step may declare) | `60000` | yes |
| `execution.capture.backoff_enabled` | boolean, whether a step's wait for a page lengthens its pause over unchanged frames | `true` | yes |
| `execution.capture.idle_ceiling_ms` | milliseconds `50..=60000`, the longest pause the back-off reaches unless the declared interval is longer | `5000` | yes |

A value outside its range is `config_value_invalid` (`out_of_range`, with `range_min` and
`range_max`) at start, in `check-config` and in `ConfigureSettings`; a value of another type is
`wrong_type`. The rows are those of `config.parameters.execution`. Each run's effective
intervals stay where they were recorded (`EffectiveTaskTiming.capture_interval`, each operation's
`interval`); a pause the back-off lengthened is the span of its wait boundary
(`page_recognition_wait`, `postcondition_wait`).

## The `install` section (Workflow #421 INST-1)

Workflow #421 ([install-cli.md](install-cli.md)): the tunables of `actingctl install`, the
installer of a single-location root (Alice 10-09: 「绝不硬编码」). actingd parses and checks the
section with the rest of the file (its rows are those of `config.parameters.install`) but never
uses it. `actingctl install` reads it from the root's configuration file at every command,
through this crate (`install::settings`), never from the configuration in effect, so no key is
settable. `actingd-config` revision 17 ([component-interfaces.md](component-interfaces.md), Runtime
v0.12.18) adds the section; a file that does not name it keeps its revision. Revision 18 (Runtime
v0.12.19, Workflow #421 INST-2) adds the keys marked "revision 18" below and retires
`install.guard.blocked_windows`. The section and each of its objects deny unknown members.

| Key | Type | Default (`actingd.defaults.json`) | Read by |
|---|---|---|---|
| `install.lock_wait_ms` | milliseconds `1000..=600000` | `60000` | the wait for `install\writer.lock` |
| `install.stop.drain_timeout_ms` | milliseconds `1000..=3600000` | `300000` | the hold's drain (INST-2) |
| `install.stop.shutdown_wait_ms` | milliseconds `10000..=600000` | `60000` | the close (INST-2) |
| `install.start.ready_timeout_ms` | milliseconds `60000..=3600000` | `900000` | the installer start's readiness (INST-2) |
| `install.start.attempts` | integer `1..=5` | `2` | start attempts after an error terminal outcome (INST-2) |
| `install.hold_after_start` | boolean | `true` | a composite update ends held (INST-2) |
| `install.hot.idle_wait_ms` | milliseconds `0..=3600000` | `600000` | hot placement's idle wait (INST-4) |
| `install.rollback.open_margin_ms` | milliseconds `0..=600000` | `20000` | the rollback gate's open-time part |
| `install.guard.min_available_memory_mb` | integer `0..=1048576` | `10000` | guard `memory` (INST-2) |
| `install.guard.before_next_start_ms` | milliseconds `0..=86400000` | `900000` | guard `schedule_near`, informational: what is due within it is listed, never refused (INST-2; Alice's decision 75 of 10-27) |
| `install.guard.blocked_windows` | list | `[]` | **retired in revision 18** (since v0.12.19, Alice's decision 75 of 10-27): accepted when it is a list, no effect; every `actingctl install` command notes it (below) |
| `install.guard.adb_timeout_ms` | milliseconds `1000..=60000` | `15000` | guard `adb` (INST-2) |
| `install.child.check_config_timeout_ms` | milliseconds `10000..=3600000` | `900000` | each `actingd check-config` the installer runs (INST-1b-F fix round 1) |
| `install.child.ledger_read_timeout_ms` | milliseconds `10000..=3600000` | `300000` | each `actingledger` read: S0, each page of the rollback gate's reader part (INST-1b-F fix round 1) |
| `install.child.ledger_verify_timeout_ms` | milliseconds `60000..=86400000` | `1800000` | the `actingd ledger-maintenance verify` of an update or rollback (INST-1b-F fix round 1) |
| `install.mcp_entry.poll_ms` | milliseconds `100..=60000` | `2000` | how often the MCP entry looks at the install (MCP-ENTRY, through `actingctl mcp-entry plan`) |
| `install.mcp_entry.settle_ms` | milliseconds `0..=600000` | `10000` | how long a change must stay put before the entry asks for a plan (MCP-ENTRY) |
| `install.mcp_entry.verify_timeout_ms` | milliseconds `1000..=600000` | `60000` | a candidate child's verification, the plan and a handle count (MCP-ENTRY) |
| `install.mcp_entry.drain_max_ms` | milliseconds `0..=86400000` | `600000` | how long a switch waits for outstanding handles before its status says so; it keeps waiting (MCP-ENTRY) |
| `install.mcp_entry.refused_retry_max_ms` | milliseconds `1000..=86400000` | `300000` | the cap of the entry's wait before the next plan after consecutive refusals; the wait doubles from `settle_ms` (MCP-ENTRY fix round 1) |
| `install.start.sample_interval_ms` | milliseconds `1000..=600000` | `30000` | how often the installer's start journals a progress sample: the start log's size, the starting Runtime's reads (INST-2, revision 18, since v0.12.19) |
| `install.start.stall_after_ms` | milliseconds `60000..=3600000` | `720000` | the stall rule's detection: a start log that did not grow for this long is the caveat `start_stalled`; nothing is killed (INST-2, revision 18, since v0.12.19) |
| `install.start.stall_read_bytes` | integer `0..=1099511627776` | `53687091200` | the stall rule's detection: a starting Runtime that read more than this before it was ready is the caveat `start_stalled`; nothing is killed (INST-2, revision 18, since v0.12.19) |
| `install.start.memory_wait_ms` | milliseconds `0..=3600000` | `120000` | how long a start attempt waits for guard `memory` to pass (the pages of a Runtime that just stopped are released over time) before it refuses (INST-2 fix round 1, revision 18, since v0.12.19) |
| `install.stop.busy_wait_ms` | milliseconds `0..=3600000` | `300000` | how long the hold waits for an instance's queued requests to clear when the Runtime refuses its pause, before it pauses it again once (a run in flight is left to the pause's own drain) (INST-2 fix round 1, revision 18, since v0.12.19) |

A child still running at its `install.child.*` deadline is left running, never killed (the
stall rule), and reported with its `pid` and the key ([install-cli.md](install-cli.md),
"Children and their deadlines").

`install.guard.blocked_windows` is retired in `actingd-config` revision 18 (Runtime v0.12.19,
Alice's decision 75 of 10-27: there is no allowed or blocked deploy time span; the hold records
and hands back the deferred work instead). A file may still name it: a list is accepted and
has no effect, its windows are not judged, and every `actingctl install` command that reads
the section notes it as `install_caveat_noted` (`install_caveat` `config_key_retired`,
`config_key` the key); a value that is not a list is `config_value_invalid` (`wrong_type`). The
defaults file keeps `[]`, which is not noted. The Mon/Thu restart is no longer data in this key.

A value outside its type or range is `config_value_invalid` (`out_of_range` with `range_min` and
`range_max`, or `wrong_type`) at actingd's start, in `check-config` and for every `actingctl
install` command that reads the section; an unknown member of the section or of one of its
objects for `actingctl install` is `config_value_invalid` (`section_check`) for its key. actingd refuses an unknown member of the section as it refuses any
unknown member of the file (`config_decode_failed`).

## The `idle` section (IDLE-1)

IDLE-1 (Alice's ruling 41 of 10-24: when an instance has no task, close the game inside the
emulator, and the startup package relaunches it when a task is due; her ruling of 10-25:
「没有任务的时候直接关闭模拟器……任务结束或者接下来一段时间比如30分钟内没有任务的时候关闭应用程序」, two options
of one function; Workflow #386 comment 6393646823): what the Runtime does with an instance that
has no task soon, and when it brings the instance back. Each key is also per instance,
`instance.<instance id>.idle.<member>`, in the file the `idle` member of the `instances[]` entry,
default the section's key of the same name (`section:idle.<member>`). Every key is settable: the
Runtime reads them at every idle check, so an instruction takes effect at the next one; an
instance that is idle when its action in effect changes is brought back at once. `actingd-config`
revision 12 ([component-interfaces.md](component-interfaces.md)) adds the section and the entry's
member; a file that names neither keeps its revision.

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `idle.action` | one of `none` (nothing ever happens), `close_app` (the game's application is stopped inside the emulator), `stop_emulator` (the instance's sessions are closed and its emulator is stopped) | `close_app` | yes |
| `idle.min_gap_ms` | milliseconds `60000..=86400000`, the least time to the scheduler's next due task for the instance before the action runs | `1800000` | yes |
| `idle.close_app_wake_lead_ms` | milliseconds `0..=3600000`, how long before the next due task an instance whose application was closed is brought back (the startup package); the default covers a game's cold start (80 to 141 s in production on 10-27; #405 IDLE-1P, it was `60000`) | `300000` | yes |
| `idle.stop_emulator_wake_lead_ms` | milliseconds `0..=3600000`, how long before the next due task an instance whose emulator was stopped is brought back (the emulator's boot and the startup package) | `300000` | yes |

A value outside its range is `config_value_invalid` (`out_of_range`, with `range_min` and
`range_max` for the milliseconds) at start, in `check-config` and in `ConfigureSettings`; a value
of another type is `wrong_type`. The whole-section check, the section's and each instance's keys in
effect: each wake lead is shorter than `idle.min_gap_ms` (`section_check` on the lead's key), and
an instance whose action in effect is `stop_emulator` is one whose emulator the Runtime starts
(`instance.<id>.start_emulator` true; `section_check` on `instance.<id>.idle.action`), since its
wake starts the emulator through that configured start. The rows are those of
`config.parameters.idle` and, per instance, `config.parameters.instance`.

When (the Runtime host, `host/idle.rs`): at every policy cycle, right after its evaluation and
before actingd admits its dispatches, and between cycles every second of the lease sweep, the idle
check takes each registered physical instance the configuration names. The action runs once when
the instance has no lease, no queued request and no keyless work holding its admission guard, no
scheduling pause holds it (nor a resource fence), its emulator runs, it can be brought back (a
startup package, and for `stop_emulator` its configured emulator start), and the latest policy
evaluation, made after the instance was last seen in use, has no due task for it and no next wake
closer than `idle.min_gap_ms` (the not-run explanation's due candidates and next wake of the
instance, the evaluator's own; no other estimate). It runs on an emulator-control claim of the
instance's key. `close_app` stops the application `<standard_packs.root>/<game>/applications.json`
declares for the instance's server (`servers.<server>.application_id`), which must be the
instance's assigned application, through the application lifecycle path; `stop_emulator` is the
emulator-control Stop on the claim's key (the fence on the key, the retained session's close, the
provider's Stop, never MuMuManager `api`). The wake comes at the next due time minus the action's
wake lead, or at once when a later evaluation shows a due task: after `close_app` the startup
package is queued, after `stop_emulator` the configured emulator start, which hands the key to the
startup package. While an instance's emulator is held stopped the emulator watch does not start
it.

What wakes an idle instance (IDLE-WG, since 0.12.27; production v0.12.26 on 10-29 02:43: an
instance resume's reconnect claim held an idle instance, its candidates waited
`dispatch_instance_held` with no wake, and the plan woke the closed game for tasks its closed
activity window then refused, one startup package spent for nothing): a due task is one that could
be dispatched now, a selected decision or an eligible one with no wake of its own later than the
evaluation, on an instance no blackout covers ([scheduling-blackout.md](scheduling-blackout.md))
and whose activity window is open at the evaluation. So the wake comes at once for a task really
due, for a switched action, for a wake whose time has come (also one that fell due while a pause,
an install hold or a claim held the instance, whose claims then wait behind the holder), and for
the start's owed wake (an instance restored woken, or with no usable record). It does not come for
a task a time-bound gate holds back: a blackout that covers the instance, or its closed activity
window (`policy_activity_window_closed`), also when its decision names a hold that comes before
those gates in the order of the deferrals (a scheduling pause, `dispatch_instance_held` for a claim
on the instance's key such as a resume's reconnect). Such a task moves the wake only to the time
the gates let it through, the blackout's end and then the next opening of the activity window,
minus the action's wake lead, and an earlier wake in effect stays. The evaluation's decisions and
the not-run explanation are unchanged; only the plan the idle check reads counts the gates. Each action and each wake is one `instance_idle_action` record (`operation` `run_idle_action`
or `queue_idle_wake`, `idle_action` naming the action). An instance whose action in effect cannot
bring it back (its `instances[]` entry has no `startup_package`, or for `stop_emulator` no
`start_emulator`) never acts, and the first idle check that finds it records one
`instance_idle_action_unsupported` warning (`member` naming what the entry lacks, `operation`
`check_idle_instances`) and the same report line, once per action in effect; after a start that is
the first policy cycle or lease sweep. A failed close or stop is recorded and goes through the one
decision point (`contain_failure`, #419), so it fails only that instance's action: the instance is
left as it is, no wake follows, its next due task goes through the normal path, and only a failure
that is the Runtime's as a whole latches it.

Across a start (#405 IDLE-1P; on 10-27 a restart at 17:32 forgot a game closed since 14:35 and its
17:54 wake, and its 18:01 task met the closed game): every change of the instances the action
closed, or is closing, is recorded as the runtime fact `host.idle_state` (scope Runtime, a record
list: first a writer row naming the recording start's `start_anchor` alone, then one row per
instance: `instance_alias`, `action`, and `wake_at_unix_ms` when a wake is planned; tagged in
`ledger_observer.tag.workbench_patterns`, so a start loads it,
[runtime-fact-store.md](runtime-fact-store.md)). Since 0.12.25 (#405 N-2) the writer row names
the start anchor, the start event sequence of the start that wrote it as `host.contained_settlement`
names it, instead of the owner epoch: an owner that ran no Runtime start to its settlement in
between (an offline ledger maintenance, a deploy's ledger check, a start that failed before its
settlement) no longer makes the record stale. A woken instance stays in the record until its
startup package has run (#405 O-2, since 0.12.25): its wake only queues claims, which wait in
memory behind a scheduling pause or an install hold, so its row says `woken` (true) with the time
it was woken as `wake_at_unix_ms`, and the instance is no longer kept once its wake's claims have
ended (its startup package ran, or the claims ended without it: a claim an emulator control
cancelled) and an idle check finds it free. A wake's configured emulator start that finds the
emulator already running (after `stop_emulator`, a restart that came once the wake had started it)
ends as `emulator_autostart_skipped` and still queues the startup package once, as its Start would
have handed it the key ([emulator-control.md](emulator-control.md)). A start restores them from a
record the last start that completed its settlement wrote (the anchor `host.contained_settlement`
named before this start), as the scheduling pauses are restored and before any instance is
prepared, and records them again under its own anchor: each comes back
idle, owing its wake, which the start's first idle check queues when its time has passed or a plan
shows a task due, so an instance restarted within its wake lead is brought back at once, and a
woken one whose startup package had not run is woken again at once. An action in flight at the
stop counts as done. After an earlier start that kept no record, with a record another start wrote
(a start that kept none ran in between, such as a rollback's older build, whose idle action was not
recorded), or with a record of another shape (`invalid_runtime_state_fact`, operation
`restore_idle_state`, recorded, and the start goes on; a record a build before 0.12.25 wrote under
its owner epoch is one), the start does not know what the action left: every instance whose action in effect
could have run on it (its startup package, and for `stop_emulator` its emulator start) counts as
closed, never as open, and owes its wake at the start's time (review N-1): the start's first idle
check wakes it once, its startup package waits behind a scheduling pause or an install hold as
after every wake (for `stop_emulator` after its configured emulator start, which a pause never
holds, [emulator-control.md](emulator-control.md)), and the next plan
closes it again with its proper wake. So an instance whose only work waits for a status reading
that expired meanwhile is not left closed with no wake. The configured emulator start at daemon
start leaves an emulator the action holds stopped to its wake.

The scheduler's fact refresh on a closed instance (#405 IDLE-1P): its recognition maintenance
package (R_I, [scheduling/README.md](scheduling/README.md)) is held back until the instance's next
natural wake unless a candidate it would read for is due (its trigger true): R_I's decision carries
`instance_idle_action` and the waiting candidates `fact_refresh_pending` with `gate_code`
`instance_idle_action` and no wake. When one of R_I's own gates holds it as well (its minimum
interval, its failure backoff, a limit), the candidates name that gate instead, and its wake stays
on R_I alone: no wake of R_I's becomes a candidate's, so the end of R_I's cooldown never wakes the
closed game. The idle check reads the plan's tasks and its
reading apart: a reading alone neither wakes an idle instance nor sets its wake (a task due that
waits for a reading does), while on an active instance a reading due, or one within
`idle.min_gap_ms`, keeps it from the action as a task does. The daily limit and the other gates of
R_I are unchanged.

## The `display_safe_area` section (Workflow #392 DA-4b-D)

Workflow #392 DA-4b-D (Alice's ruling 69 of 2026-10-26: 「只有安全区适配不做，但是需要留出来声明式的安全区接口，
也就是说可以依照声明的文件直接切换安全区形态」, 「一份文件对应一个机型或多个机型，并且修改设置后实时生效」;
[display-adapter.md](display-adapter.md) section 9): where the declared safe areas live, how often
the Runtime reads them, and which device model an instance declares. The model is also per
instance, `instance.<instance id>.display_safe_area.model`, in the file the `display_safe_area`
member (`{model}`) of the `instances[]` entry, default the section's key
(`section:display_safe_area.model`). Every key is settable: the Runtime reads them at every frame
of a run whose game's display declaration measures on the safe area, so an instruction takes
effect from the next frame. `actingd-config` revision 15
([component-interfaces.md](component-interfaces.md)) adds the section and the entry's member; a
file that names neither keeps its revision.

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `display_safe_area.directory` | string, the directory of the declaration files (`actingcommand.display-safe-area.v1`); a relative path resolves against the Runtime's state root | `display-safe-area` | yes |
| `display_safe_area.check_interval_ms` | milliseconds `0..=3600000`, the least time between two reads of the directory; 0 reads it at every frame that needs it | `1000` | yes |
| `display_safe_area.model` | string, the device model every instance declares unless it names its own; empty declares none | empty | yes |

A value of another type is `config_value_invalid` (`wrong_type`); an empty directory or an interval
outside its range is `out_of_range`, at start, in `check-config` and in `ConfigureSettings`. The
rows are those of `config.parameters.display_safe_area` and, per instance,
`config.parameters.instance`. What the declarations hold, which insets a frame takes and how a
broken declaration is refused (`display_map_unresolved` with its `display_problem`) is
[display-adapter.md](display-adapter.md) section 9.

## The `host` section (RB-2)

RB-2 (Alice's ruling of 2026-10-29 10:2x, 「设回正常」; [v012-register.md](v012-register.md)
§1.202): the memory priority and the I/O priority actingd gives its own process. After the
machine's reboot of 2026-10-29 05:23 the watchdog's scheduled task (task priority 7) started
actingd, and the process inherited the task's memory priority 2 and I/O priority Low: no code set
them. The operating system evicts the pages of a low memory priority first, so the ledger's cached
pages (a 7.7 GB file) never stayed in memory and its reads went to disk for hours (6 600 to 8 600
physical reads a second on its drive); an actingd a deploy starts had normal priorities and did not
show this. So at every start, right after it read its configuration and before the Runtime host
starts (before the ledger opens), actingd sets both priorities of its own process to these keys'
levels and reads them back. **The CPU priority class stays as the process was started** (below
normal under the watchdog's task): no key names it and the start never changes it.

| Key | Type | Default (`actingd.defaults.json`) | Settable |
|---|---|---|---|
| `host.process_memory_priority` | string `normal` \| `low`: Windows' memory priority 5 (`MEMORY_PRIORITY_NORMAL`) or 2 (`MEMORY_PRIORITY_LOW`), set with `SetProcessInformation(ProcessMemoryPriority)` and read back with `GetProcessInformation` | `normal` | no (applied before the ledger opens) |
| `host.process_io_priority` | string `normal` \| `low`: Windows' I/O priority hint 2 (`IoPriorityNormal`) or 1 (`IoPriorityLow`), set with ntdll's `NtSetInformationProcess(ProcessIoPriority)` and read back with `NtQueryInformationProcess`, both resolved at run time | `normal` | no (applied before the ledger opens) |

- **Read at start only.** Both keys are read once, from the configuration in effect at the start
  (the file and the defaults file; no instruction value is known before the ledger opens), so
  `ConfigureSettings` refuses them (`config_key_not_settable`); the file changes them at the next
  start. `actingd-config` revision 20 ([component-interfaces.md](component-interfaces.md)) adds
  the section; a file that does not name it keeps its revision.
- **Refusal.** A value that is no string is `config_value_invalid` (`wrong_type`), a string other
  than `normal` or `low` `config_value_invalid` (`out_of_range`), each naming its key (`config_key`),
  at start and in `check-config`. `check-config` checks the section and applies nothing.
- **The I/O priority call.** Windows documents no call that sets a process's I/O priority: its
  `SetPriorityClass(PROCESS_MODE_BACKGROUND_END)` only ends a background mode the process entered
  itself (a process of a below-normal task is in none), and `SetFileInformationByHandle`
  (`FileIoPriorityHintInfo`) sets one file handle's hint. The native class `ProcessIoPriority` has
  kept its number and values since Windows Vista. What was observed: on 2026-10-29 the actingd
  the watchdog's scheduled task started read I/O priority Low (1) through this class. Raising a
  process's own I/O priority to normal needs no privilege. Its risk: a later Windows may change or
  refuse it; then the call fails with its NTSTATUS, recorded as below, and the start goes on.
- **Record.** The start writes one outcome as its daemon lines (`OUTCOME actingd:`), at once,
  before the ledger opens: `host_process_priority_applied` (info) when every call succeeded and
  each level read back is the one asked for, else `host_process_priority_failed` (warning). Both
  carry `memory_priority_requested` and `io_priority_requested` (the Windows levels above) and,
  where the read succeeded, `memory_priority_read` and `io_priority_read`; each failed call is a
  `caused_by` `foreign_os_error` link whose `operation` names it (`process_memory_priority_set`,
  `process_memory_priority_read`, `process_io_priority_set`, `process_io_priority_read`; a native
  call's NTSTATUS is its `raw_code`, `0x` and eight lowercase hex digits such as `0xc0000022`).
  Memory is set and read first, then I/O; a failed call does not skip the next. **A failure is
  never fatal**: the Runtime starts with whatever priorities the process has.
- **Elsewhere.** On an operating system without these priorities (not Windows) nothing is set
  or read, and the record is `host_process_priority_applied` with the two levels asked for and no
  level read. The tests apply the section through a fake of the same calls.

## Facts

| Fact (scope `runtime`) | Rows | Recorded |
|---|---|---|
| `config.subsystems` | unchanged; Workflow #384 D2-S2 adds the row `recognition_packages` when the configuration binds a recognition package: enabled when the start kept one, with one `note` link of `fact_reader_binding_ignored` per binding it runs without; SCH-M CF-1 adds the always-on row `condition_edges` (the catalog edge members, [actingd-check-config.md](actingd-check-config.md)); SCH-M CF-2 adds the always-on row `resource_reading_time` (completion-time readings, [actingd-check-config.md](actingd-check-config.md)); #386 CF-4 adds the always-on row `fact_reference` (a `fact` predicate that compares with another fact plus an offset, [actingd-check-config.md](actingd-check-config.md)); #386 CF-5 adds the always-on row `pool_fact_reference` (a pool's capacity and a `resource_projection`'s value read from a fact, [actingd-check-config.md](actingd-check-config.md)); Workflow #411 P19 adds the always-on row `reset_exclusions` (a timeline event's `excludes_fact_prefixes`, and a reset keeps a task's own last result, [actingd-check-config.md](actingd-check-config.md)); SCH-M ED-2 adds the always-on row `expected_runtime` (the run-duration samples and E, [actingd-check-config.md](actingd-check-config.md)) | at start |
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
