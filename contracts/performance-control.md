# Performance control

The performance balance controller derives one Runtime-wide control level from
the host responsiveness and third-party pressure samples of each performance
tick. Escalation, recovery, hysteresis, the transition cooldown and clock-jump
handling belong to the controller and are not changed by the consumers below.
Every instance with an active policy workload carries its own level: an
escalation below Suspended raises every instance to the new level, a recovery
releases one instance per transition. The levels in rank order are Normal,
DispatchPaused, Throttled, YieldRequested, QosReduced, Suspended and
ShutdownRequested.

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
admission. The urgency rules are unchanged: whenever a dispatch is deferred,
urgency of 950 or more fails with `performance_capacity_deadline_conflict` and
urgency of 750 or more records a `PerformanceBalanceChanged` event with reason
`DeadlineConflict`, the instance id, the instance's effective level as both
previous and current level, and the deadline disposition
(`InformationWarning` from 750, `CapacityFailure` from 950). Below 750 the
deferral fails with `performance_contention_dispatch_deferred` and records no
balance event. Every deferral is nonfatal (`InvalidRequest`) and is recorded
as the `PolicyDispatchRejected` fact that names the instance and the code.

- Normal: allowed; any open throttle window of the instance is closed.
- DispatchPaused, Suspended, ShutdownRequested: deferred with the codes and
  events above; any open throttle window of the instance is closed.
- Throttled, YieldRequested, QosReduced: the dispatch may start no earlier
  than `throttle_delay_ms` after the instance's first throttled attempt. The
  first attempt opens a per-instance throttle window ending at
  `now + throttle_delay_ms` and is deferred with the codes and events above;
  the returned error carries `retry_after_ms` (the wait still owed) and its
  rejection record carries `next_eligible_unix_ms = now + retry_after_ms`. An
  attempt at or after the window end is allowed and closes the window; an
  earlier attempt is deferred with the remaining wait. A window that was opened
  under a different level, or a remaining wait longer than the level's own
  delay (a clock anomaly), restarts the window with the current delay instead
  of granting. The driver's next policy cycle retries with a fresh decision.

Capacity admission follows an allowed gate unchanged. The resource overlay
(`apply_to_resources`: budget scaling and the heavy-dispatch cap by the global
level) is unchanged.

## Lease admission

`acquire_lease` refuses a new Business lease while the instance's directive has
`suspend_requested` or `shutdown_requested`, with the nonfatal request code
`lease_refused_performance_control` (`InvalidRequest`). The refusal is
recorded as `LeaseRequested` followed by `SchedulerDenied` with the request's
instance link, the same trail as a scheduler refusal, and the scheduler never
admits the lease. Lower levels never block a lease. Replays that return the
existing lease, renewals and resource-close-only (Drain) leases do not pass
through this gate.
