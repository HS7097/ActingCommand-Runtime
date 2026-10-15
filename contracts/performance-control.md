# Performance control

The performance balance controller derives one Runtime-wide control level from
the host responsiveness and third-party pressure samples of each performance
tick; a tick without a fresh sample follows "Recovery without evidence" below.
Escalation, recovery, hysteresis, the transition cooldown and clock-jump
handling belong to the controller and are not changed by the consumers below.
Every instance with an active policy workload carries its own level: an
escalation below Suspended raises every instance to the new level, a recovery
releases one instance per transition. The levels in rank order are Normal,
DispatchPaused, Throttled, YieldRequested, QosReduced, Suspended and
ShutdownRequested.

The controller's `clock_jump_threshold` and its heavy-dispatch limits come from
the configuration (Workflow #385 P-c): `performance.pacing.clock_jump_ms`
(default 30 000) and `performance.pacing.heavy_dispatch_limit.normal` /
`.paused` (defaults 2 and 1), read once when actingd assembles the host
([runtime-config.md](runtime-config.md), "The performance section"). The
escalation and recovery samples and the transition cooldown stay the
controller's own values.

## Responsiveness input

Host responsiveness measures execution spans: `recognition_latency_ms`
(1,000 ms target), `action_effect_latency_ms` (1,500 ms), `touch_response_us`
(100,000 us) and `capture_acquire_us` (250,000 us). Each measured span scores
10,000 at or below its target and `floor(target * 10,000 / actual)` above it;
the monitor uses the lowest score from fresh signals within the latest 64
pipeline entries. Freshness remains twice the sample interval, and consumed or
expired signals do not supply another observation.

`frame_gap_ms` describes frame arrival cadence, including declared observation
waits and time without work. `capture_latency_ms` is the event-to-event capture
time, including backend open and reopen. Both remain in stutter events, the
context's `max_frame_gap_ms` and `max_capture_latency_ms` (also in a policy
failure's `perf_context`), the bounded context queue, the summary and related
diagnostic event chains. Neither contributes a responsiveness score. A signal
containing only these two supplies no responsiveness measurement: the field
remains `None`, and a control observation
exists only when another fresh scoring signal or usable system-pressure sample
exists. System pressure keeps its own source timestamp and coverage rules;
unavailable process coverage remains unknown. The original sampler-failure,
idle-recovery, controller and admission rules below continue to apply.

## Recovery without evidence

Every performance tick reaches the controller. A tick without a fresh sample
changes no level and no transition evidence while the Runtime has owned work:
a pipeline sample within the control freshness window (twice the sample
interval), a lease held on any instance, or an admitted policy dispatch.
Without owned work, and while the global level or any instance level is above
Normal, such a tick counts as one recovery sample toward Normal under the same
`recovery_samples`, transition cooldown, one level per transition and one
instance per transition as a measured recovery; instances then have no
arbitration input, so instance id order decides. Its
`PerformanceBalanceChanged` events carry reason `Recovery`, `recovery: true`
and neither `host_responsiveness_basis_points` nor
`third_party_pressure_basis_points`; a measured recovery always carries at
least one of them. This is a liveness exit, not a measured zero: an unknown
pressure stays unrecorded.

A clock jump is measured against the controller's latest input, a sample or
a tick without a fresh sample: a tick earlier than the previous tick, a
sample earlier than the previous sample, or an input more than
`clock_jump_threshold` after the latest input. Silence between samples while
ticks continue is not a clock jump; a clock jump found on a tick without a
fresh sample carries no evidence fields.

## Host arbitration

Each workload carries the host's arbitration input for its instance (Workflow
#308 slice 5c): `utility_milli`, the highest `effective_milli`, and `aging_ms`,
the longest `aging_ms`, over the instance's ranked Eligible or Selected
decisions in the latest policy cycle that had any. The input is memory-only;
an instance no cycle has ranked yet has none.

- An escalation step into Suspended or ShutdownRequested raises exactly one
  instance per transition that passes hysteresis and cooldown: among the
  instances below that level, the one with the lowest `utility_milli` (ties:
  the shortest `aging_ms`, then the instance id). The global level stays one
  level below until no instance is below the step; the transition that raises
  the last one also moves the global level. Without instances the global level
  moves at once.
- A recovery releases, among the instances above the global level, the one
  with the longest `aging_ms` (ties: the highest `utility_milli`, then the
  instance id), still one per transition and one level at a time.
- When any candidate lacks an arbitration input, the instance id order decides
  (the order before slice 5c).
- An instance raised above the global level keeps that level when its workload
  ends (it then has no arbitration input) until a recovery lowers it; only an
  instance at or below the global level leaves the controller with its
  workload.

Every per-instance suspend or recovery `PerformanceBalanceChanged` event
carries `arbitration: {utility_milli, aging_ms, candidates, basis}`: the chosen
instance's input (0 for a value it lacks), the number of candidates, and
`basis` `utility` or `lexical_fallback`. The field is absent on every other
event; a reader that denies unknown fields must accept it.

## Directive

The directive for an instance is `level = max(instance level, global level)`,
`throttle_delay_ms = (rank - 1) * 50` (DispatchPaused 0, Throttled 50,
YieldRequested 100, QosReduced 150, Suspended 200, ShutdownRequested 250) and
the derived flags `yield_requested` (YieldRequested or higher), `qos_reduced`
(QosReduced or higher), `suspend_requested` (Suspended or higher) and
`shutdown_requested` (ShutdownRequested). An instance without its own entry
uses the global level. An empty instance id, a poisoned controller lock or
missing controller state is an error; nothing is allowed by default.

## Policy dispatch gate

`admit_policy_dispatch` consults the directive of the intent's instance after
trusted-dispatch authorization and approval projection and before capacity
admission.

- Normal, DispatchPaused, Throttled, YieldRequested, QosReduced: allowed, on
  every attempt. No level opens a throttle window, and the gate gives no
  `retry_after_ms`.
- Suspended, ShutdownRequested: deferred with
  `performance_contention_dispatch_deferred`. The deferral is nonfatal
  (`InvalidRequest`) and is recorded as the `PolicyDispatchRejected` fact that
  names the instance and the code. The driver's next policy cycle retries with
  a fresh decision.

Urgency never changes the gate's outcome: a deferral has the same code at any
urgency, never fails as a deadline conflict, and records no
`PerformanceBalanceChanged` event. Urgency only orders candidates in the
evaluator.

Capacity admission follows an allowed gate unchanged. The resource overlay
(`apply_to_resources`) does not scale the host budgets by the level: they only
express capacity. Its heavy-dispatch cap by the global level is unchanged.

## Lease admission

`acquire_lease` refuses a new Business lease while the instance's directive has
`suspend_requested` or `shutdown_requested`, with the nonfatal request code
`lease_refused_performance_control` (`InvalidRequest`). The refusal is
recorded as `LeaseRequested` followed by `SchedulerDenied` with the request's
instance link, the same trail as a scheduler refusal, and the scheduler never
admits the lease. Lower levels never block a lease. Replays that return the
existing lease, renewals and resource-close-only (Drain) leases do not pass
through this gate.

## Shadow measurement (Workflow #385 P-m)

Beside the controller above, which it changes in nothing, the performance monitor runs the
shadow measurement of the frozen pacing model (PERF v1.4.2 §3, §4, §4.6, §10.3). It measures,
evaluates both views and records; it acts on nothing. `performance.pacing.mode` stays
`observe` in every release, and enabling `pace` or `full` is the person's decision
(coordinator ruling 7 of wave 13).

- **Samples.** The host timer probe (`performance.responsiveness.host_probe.timer.*`) runs
  beside an enabled monitor: every `interval_ms` its own thread sleeps `burst` times for
  `period_ms` and hands each oversleep in microseconds over. Per registered instance, the
  fan-out point hands over each `capture.completed` `capture_acquire_us` and each
  `input.committed` `touch_response_us`; a touch response counts only for a tap (`input.tap`)
  unless `signals.touch_response.score_gestures`. A backend open that passed names the backend
  an instance's capture and touch baselines are kept for (`adb_screencap*` is `adb`).
- **Scores.** A sample scores 10 000 at or below `baseline × allowance_milli / 1000 +
  slack_us`, else `10 000 × allowance / sample`. A signal's value is the mean score of its
  samples within `window_ms`, unknown below `min_window_samples`. `R_host` is the lowest known
  host signal, `R_emu(i)` the lowest known signal of instance i; each is smoothed once per
  evaluation (`ewma_alpha_milli`) and becomes unknown when no sample came for `emu_stale_ms`.
  An instance's input is `R(i) = min(R_host, R_emu(i))` of those known.
- **Baselines.** Per scope, signal and backend: the initial value (`initial_baseline_us`, by
  backend word, else `default`) until `baseline.min_samples` calm samples, then their
  `baseline.quantile` over the last `baseline.window_samples`, held within `[initial ×
  clamp_min_milli, initial × clamp_max_milli] / 1000`. A sample is calm when its scope's stage
  is S0 and no pressure of the floor is active. A held baseline is the warning
  `perf_baseline_clamped`. Baselines stay in memory until the ledger observer's tags and
  start load (AO-5, AO-6); a restart begins again from the initial values.
- **Tracked value.** Each scope (the host, each instance) tracks `R_eff`, 10 000 at start. A
  lower input held `degrade_sustain_ms` becomes `R_eff`; an input at least `hysteresis_bp`
  higher held `recover_sustain_ms` makes `R_eff` that input minus `hysteresis_bp`. An unknown
  input counts as a recovery sample at 10 000 (cause `idle_recovery`). A clock jump
  (`pacing.clock_jump_ms`) clears the timers and changes no value.
- **Curve and views.** The curve (`pacing.curve`, `linear` or `step`) is read at `R_eff`: the
  stage of the lower neighbour, the factor and share between the neighbours or the lower one's; a
  share of 0 is S4. A pressure of a floor kind at or above `pressure_floor.min_severity` holds
  the stage at S1 at least and the factor at `pressure_floor.factor_milli` at least, never
  lowering the share. The normal view is that reading. The learning view caps its stage at
  `calibration.learning_max_stage` with a full share, and gives S4 with share 0 once `R_eff`
  stayed at or below `learning_backstop_bp` for `pause_sustain_ms`; under a person's curve, with
  `calibration.apply = off` or with the cap at S4 it equals the normal view. Each view's
  `allowed` is the ceiling of the registered instances' shares' sum, given to the larger shares
  first, then by alias.
- **Effect.** `observe` gives the stage S0 and the factor 1 000 to every row; `pace` would give
  the curve's stage and factor, `full` the learning view's stage while the learning view caps
  (P-a ends that phase). Nothing reads them yet: no dispatch, interval or deadline changes.
- **Records.** A changed stage of a scope is one `perf.pacing_changed` whose outcome is
  `perf_stage_changed` (info): `pacing_stage_from`, `pacing_stage_to`, `responsiveness_bp`
  (`R_eff`), `factor_milli`, `pacing_cause`, `pacing_mode`, and `instance_id` (the alias) for an
  instance, `host_responsiveness_bp` and `emulator_responsiveness_bp` when known,
  `pacing_learning_capped` when `full`'s learning view caps it. `perf.summary` carries the
  `pacing` members (register P4) of the last evaluation: the host's values, both views with
  their `allowed`, and each instance's views (at most 64 rows).
- **Status read.** `PerformanceStatus` (register §1.61 W15) answers from memory, reading and
  writing nothing in the ledger: the mode, the phase under `full`, whether the learning view
  caps, the monitor's health, the host row and one row per registered instance (their
  responsiveness, the curve's stage and factor, both views with their stage, share, `allowed`,
  dwell per stage over the last 24 h and in all and how often each would have stopped work, and
  the stage and factor in effect), and the warnings now (`perf_probe_unavailable`,
  `perf_baseline_clamped`). A monitor without counters answers its rows at their start with
  the health `unavailable`. `actingctl perf-status` prints it; `ac_overview` answers it as its
  block `performance`.
- **GPU.** The GPU utilisation is summed per engine type and the busiest type counts
  (`performance.pressure.gpu_aggregate = max_engine_type`, the default); `sum` keeps the earlier
  sum of every engine.
- **Package load (MEM-1).** A run's task timing evidence carries `package_load_ms` and
  `package_load_bytes` (register P8): how long its main package took to load and the bytes it
  holds in memory.
