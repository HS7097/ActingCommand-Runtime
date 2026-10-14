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
