# Scheduling blackouts and complete silence

Workflow #386 (scheduling rules, frozen model v1.4.2, sections 5 and 3.2). Scheduling time is a
blacklist: an instance may run whenever no blackout applies to it and its budgets allow. A
blackout is complete silence: inside it the Runtime starts nothing on its own for the instances
it covers, and it does not react to a lost connection either. A person's, an agent's or Lab's
own run is let through. Before a blackout starts, the instance goes quiet gradually, because a
run is admitted only if it is certain to end before the start.

**Status.** This document is written ahead of its code (documentation slice SR-0). Slice SR-4a
implements the blackout and quiet checks of the policy, the recovery ladder, the startup
package, the instance monitor and the emulator autostart; slice SR-4b implements the safety
net that stops runs at a blackout start. SR-1 parses every `scheduling.blackout.*` key from
v0.12.0 on ([v0.12.0 register](v012-register.md) §5), so check-config lists them before
anything reads them; they take effect only from SR-4a. Before SR-4a2, timeline `maintenance`
events and blackout entries had no Runtime consumer: a catalog generator must not rely on
blackouts before check-config reports the `scheduling_blackout` subsystem that SR-4a2 adds
("Configuration"). Until SR-4a3 the ladder, the startup package, the emulator autostart and
the monitor probe do not yet keep the silence of "Complete silence".

**SR-4a in three parts** (SCH-M frozen model v1.0 §3.2 and §6, Workflow #386, 10-24). SR-4a1
(built) adds the per-instance list, the `once` entries, the pure judgements of "Pure judgement"
and "Day-boundary fit", and their configuration keys. SR-4a2 (built) closes the policy over
them: positions 4 and 7 of "Order of the deferrals", the day boundary, the admission re-check,
the random-delay cut and the `scheduling_blackout` subsystem; with no entry in either list and
no catalog `maintenance` event no blackout defers, cuts or refuses anything (the day boundary
is its own per-instance switch, "Day-boundary fit"). SR-4a3 (built) brings
complete silence to the ladder, the startup package, the emulator autostart and the monitor
probe ("Complete silence", "Gradual quiet" rule 3). With both lists empty and no catalog
`maintenance` event, none of SR-4a3's entries reads a blackout and nothing behaves differently.

## Blackouts

### Catalog blackouts

A catalog blackout is a V2 timeline event with `event_kind` `maintenance`
(`scheduling/v2/timeline.schema.json`; the schema needs no change). It must have
`duration_ms > 0` and the V2 `validity` interval, with the same rules as a `timeline_active`
window (`scheduling/v2/README.md`). Its scope (`instance`, `server` or `game`) decides which
instances it covers; an occurrence covers `[occurrence, occurrence + duration_ms)` intersected
with the validity. `invalidates_fact_prefixes` keeps its meaning (facts observed before the
occurrence are unknown until observed again). Game rules such as reset times and maintenance
periods exist only in the catalog data that a generator writes from the resource side; the
Runtime contains none of them.

A neutral example, one hour every day from 03:30 UTC for one instance:

```json
{"id":"neutral.reset-guard","scope":{"kind":"instance","instance_id":"instance-a"},
 "event_kind":"maintenance",
 "schedule":{"kind":"daily","clock_source":{"kind":"server","timezone_id":"etc/utc",
   "utc_offset_minutes":0,"dst_offset_minutes":0,"maintenance_drift_ms":0},"minutes_of_day":[210]},
 "duration_ms":3600000,"invalidates_fact_prefixes":[],
 "validity":{"from_unix_ms":1790780400000,"until_unix_ms":null}}
```

`reset` events (zero duration, invalidation only) stay as they are; they are not blackouts.

### Configuration blackouts

Two lists of the actingd configuration hold blackout entries:

- **Host blackouts.** `scheduling.blackout.host` covers every instance of the host: planned
  machine restarts, deploys, a person's quiet period. Default empty.
- **Instance blackouts** (Workflow #386 SR-4a1). Each `instances[]` entry's
  `scheduling.blackout` (key `instance.<instance id>.scheduling.blackout`) covers that instance
  only. Any instance may have one, for any application: it is configuration data, never a rule
  in code (Alice 10-24: 「任何应用都可以通过修改文件来达到黑名单效果」). An instance whose entry names
  no list has no entries of its own; nothing in the Runtime, the defaults file or the contracts
  names a game.

Both lists are settable while the Runtime runs (`ConfigureSettings`; an instance's list once its
entry names one, even `[]`) and accept two kinds of entry:

- `weekly` (an entry without `kind` is one):
  `{kind?: "weekly", id, weekdays, utc_offset_minutes, start_minute_of_day, end_minute_of_day}`,
  the time fields having the shape and bounds of an activity window
  (`scheduling/v2/activity.schema.json`, `window`): `weekdays` 1–7 (unique, at least one) are the
  days it starts on, `utc_offset_minutes` within ±840, both minutes within 0–1439, an `end` at or
  before `start` crossing midnight (an `end` equal to `start` lasts one day).
- `once`: `{kind: "once", id, from_unix_ms, until_unix_ms}`, the interval
  `[from_unix_ms, until_unix_ms)`, non-empty (`until_unix_ms` after `from_unix_ms`).

A list holds at most 64 entries (a range, not a threshold), and `id` is unique within its list.
An entry that breaks a rule fails check-config and startup with `blackout_config_invalid`:
`field` points at the member (a JSON pointer into the configuration file:
`/scheduling/blackout/host/<n>/…` or `/instances/<i>/scheduling/blackout/<n>/…`),
`blackout_id` names the entry when it has a usable id, and `blackout_source` (`host` |
`instance`) names the list. Through an instruction the same entries are `config_value_invalid`
(`section_check`) and the configuration in effect stays. Entries have no `applies_to`: every
blackout is complete silence.

### Activity profiles become bookkeeping days

With blackouts in place, an activity profile no longer says when an instance may run. Each
instance has one daily window `[reset, reset − 1 min)`, so that its day is the game's day
(an overnight window's `local_day` is the day it opens). A window with `start == end` is not
used for this, because it always starts at local midnight. Without a blackout, an instance may
run at any time its budgets allow. The generator change that rewrites the activity profiles is
data-side (slice SR-8), not Runtime code.

## Pure judgement

The policy crate holds two pure functions next to `activity_window_at` (Workflow #386 SR-4a1,
`crates/policy/src/evaluator/blackout.rs`). They read exactly three sources, vocabulary
`blackout_source`:

| Source | What | Covers |
|---|---|---|
| `host` | `scheduling.blackout.host` | every instance |
| `instance` | the instance's own `scheduling.blackout` | that instance |
| `catalog` | the active catalog's timeline `maintenance` events (explicit V2 validity; a zero duration covers nothing) | the instances their scope matches |

- `blackout_at(sources, instance, t)` returns the blackout covering the instance at `t`
  (`{id, source, start, end}`) or none. It is the maximal union of the overlapping or adjacent
  intervals of the three sources that contains `t`. Its `end` is the union's end, so a wake at
  that end never lands inside the next interval; `start` is the union's start. `id` and
  `source` belong to the entry that covers `t` and ends last (of equal ends: `host`, then
  `instance`, then `catalog`, then the least id).
- `next_blackout_start(sources, instance, t, bound)` returns the next blackout that begins after
  `t` and at or before `bound`, or none: its start, the union's end, and the entry that covers
  its start and ends last. When a blackout covers `t`, the next one begins after its end.
- A union is followed through at most 1 024 intervals each way (a computation bound): only a
  table that covers whole weeks without a gap builds a longer one, and the evaluation at the end
  reached finds the rest.

### Day-boundary fit

`day_boundary_fit(profile, now, required_ms)` (SCH-M D-6, pure, same file) admits a
Runtime-started policy run only if `now + required_ms` is at or before the end of the activity
window it is admitted in. Otherwise it returns `{window_end_unix_ms, required_ms,
next_eligible_unix_ms}`, the wake being the start of the profile's next window; outside every
window it returns nothing (the activity window's own gate decides). `required_ms` is the run's
expected duration E (SCH-M §3.4), the intent's `expected_duration_ms`. SR-4a2 wires it as
`policy_day_boundary_approaching` (info) at position 7, after `blackout_approaching`, on
instances whose `scheduling.day_boundary.quiet` is true in effect, for every policy dispatch
(fact refresh reads included), and admission refuses an intent that no longer fits with the
same code. Shorter work still runs up to the window's end (Alice's ruling 50, 10-24: the
day-boundary rule as recommended, per-instance switch, default on). The rule needs no blackout
entry: it holds on every instance whose switch is on, also with empty lists.

## Complete silence

Inside a blackout the Runtime starts nothing that it starts on its own for a covered instance.
The rule is decided by the requester kind of the instance's one queue (`ClaimKind`,
`crates/scheduler/src/lib.rs`), never by which client asked:

| Requester | Inside a blackout | After it ends |
|---|---|---|
| Policy dispatch (including owed condition reruns, recognition readings of Workflow #384 and owed clock occurrences) | Deferred with `blackout_active`, wake at the blackout's end; admission refuses an intent that crossed into the blackout with the same code | The first evaluation dispatches as usual |
| Recovery ladder, for each trigger stage (task, startup preparation, connection preparation, recovery preparation) | Dropped, never queued: one ladder suppression with reason `blackout` and `until_unix_ms` the blackout's end. The stability-warning count of Workflow #387 counts no record made inside a blackout | Not raised again; a fault that persists triggers anew |
| A ladder claim that waits for the key when a blackout starts (including one raised by performance pacing, Workflow #385) | Dropped the same way; the condition rerun of the run it followed stays owed | As above |
| Startup package | Stays queued; it is taken out only after the blackout (a bounded wait, wake at the end) | Runs first |
| Emulator autostart (`autostart` claim: the daemon start's, also after a watchdog start; the emulator watch's restart of a configured emulator it found stopped; IDLE-1's wake after `stop_emulator`) | Held: no key, no emulator start; it stays queued with a bounded wait. The watch's polls go on (a status read starts nothing) and it requests no second claim while one waits, so its restart counts once | Starts the emulator; the startup package follows |
| IDLE-1's idle action and wake | No action starts (as under a scheduling pause); a wake due inside the blackout queues its claims, which wait as above | The wake's claims are granted; the action is planned again |
| A #403 fence's periodic recheck (a self-check the Runtime queues) | Not queued | Due again at once |
| Instance monitor probe | Takes no frame; a failed take is not recorded | Probes again when due |

A lost connection is not handled: a failed startup or connection preparation inside a blackout
is observed and recorded by the probes as usual, and raises no ladder, no reconnect and no
emulator restart. The ladder rule is code, not a ladder configuration switch (Workflow #387,
rule A7): to let a ladder climb at some time, change the blackout table.

**Client runs are let through.** Runs and operations a client asks for are instructions and are
not held by blackouts or by quieting, and the safety net does not stop them: requester kinds
`direct_task_run` and `lab_operation`. Today these are `actingctl task-run`, MCP `ac_run_pack`
and actinglab `lab run` with the Lab observe and do operations; after Workflow #389 they are the
Lab channel's option two (actinglab `run start`, `lab run`, and MCP `ac_run_pack` forwarded to
it; `contracts/runtime-interface.md`). The probes record them as usual. A ladder that such a run's
failure would raise is the Runtime's own work and is dropped inside a blackout like any other.
A person's reconnect self-check of an instance (`SelfCheckInstance`; after #389 a Lab option-two
operation) is carried out; a ladder its failure would raise is dropped.

A ladder that is climbing when a blackout approaches does not start its next rung when that
rung's time limit plus `quiet_margin_ms` reaches the blackout's start: the ladder ends with the
suppression reason `blackout` and releases the key. It is not continued after the blackout.

**How the Runtime holds them (SR-4a3).** The host reads an instance's blackout from the three
sources with the pure judgement of "Pure judgement" on the Runtime clock: the two lists in effect
now, and a copy of the active catalog's `maintenance` events that the start and every lease sweep
tick take (a `maintenance` event with a duration and no V2 validity cannot be judged: it is left
out and recorded once per catalog generation as `blackout_config_invalid` with `field` its
`validity`, its `blackout_id` and `blackout_source` `catalog`).

- The instance's routine gate (`crates/scheduler`, `ClaimGate::Silenced`) holds the startup claim
  while a blackout covers the instance or the next one starts within the package's response
  deadline plus `quiet_margin_ms` (rule 3), and the `autostart` claim while a blackout covers it.
  A held claim is never granted at once, a hand-off to it finds no candidate, it refuses no
  dedicated lease and counts as no waiting work for a shutdown (as a paused routine entry). The
  lease sweep's pump grants it at the first tick after the gate opens. An `autostart` claim
  granted before the blackout holds its key without its Start until the end.
- A ladder's admission, and a granted ladder claim's climb, drop it with
  `recovery_ladder_suppressed` `blackout` (Info, `until_unix_ms` the blackout's end). Every lease
  sweep tick drops the ladder claims queued on a covered instance the same way. A rung's time
  limit is what its configuration bounds: the return-home package's deadline
  (`configured_package_deadline_ms` for the configured package, the default response deadline
  for a bound one); `entry_hold_ms` plus the startup package's deadline; `stop_bound_ms`,
  `start_bound_ms`, `readiness_window_ms` plus the startup package's deadline. A busy or screen
  hold's rerun of the return-home rung is checked the same way. A person's drill is a ladder's
  claim and is refused `recovery_admission_denied` inside a blackout.
- Direction ① (Workflow #387) counts no record whose time lies in a blackout the lease sweep found
  covering its instance.

## Gradual quiet

1. Every policy admission requires `now + horizon + quiet_margin_ms ≤ next blackout start`,
   checked in the evaluation and again at admission. `horizon` is the run's response deadline,
   the wall-clock hard limit that also sets its lease time. It is not multiplied by the
   performance pacing factor, because pacing never lengthens the wall-clock limit (Workflow
   #385). The next start is searched only up to `now + horizon + quiet_margin_ms`. The key
   `scheduling.blackout.quiet_horizon` (Workflow #386 SR-4a1) chooses the horizon: `deadline`
   (the default, the response deadline) or `expected` (the run's expected duration E, the
   intent's `expected_duration_ms`); slice SR-4b changes its default to `expected` with the
   safety net. The response deadline (SR-4a2) is that of the request the dispatch runs under:
   admission reads the request it is admitted with; the evaluation, before any request exists,
   reads the one it will be: a fact refresh read runs its instance's recognition maintenance
   package under the request's default deadline (60 s), and every other scheduled run, of a
   configured procedure or of a composed standard pack, under the bound every such request
   carries (30 min, `ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS`). Under `deadline` a policy
   run therefore starts no later than 31 min before a blackout with the default margin.
2. A candidate that fails it is deferred with `blackout_approaching` and wakes at the end of
   that blackout; `required_ms` is its horizon. Shorter candidates still run, so the closer the
   start, the shorter the work that is admitted: there is no fixed quiet start. Admission
   refuses an intent that no longer fits with the same code, at Info (no lease, no run).
3. A startup package is taken out of its queue, and a ladder starts each rung, only under the
   same condition with its own time limit; otherwise it waits (startup package) or ends
   (ladder) as above.
4. From `start − hard_quiet_ms` (default 0, off) no new policy dispatch, ladder rung or startup
   package starts. For a policy dispatch (SR-4a2) this is part of rule 1: a candidate is
   deferred `blackout_approaching` when `now + horizon + quiet_margin_ms` passes the start or
   `now + hard_quiet_ms` reaches it.
5. A policy run admitted under rule 1 ends before the start, because its deadline is hard
   (the lease deadline is the earlier of the request deadline and the lease expiry).

## Safety net at the start

With `stop_in_flight_at_start` (default true), a run the Runtime started itself that is still
active when a blackout starts stops at its next checkpoint with `blackout_run_stopped`. Two
cases are realistic: a ladder rung or a startup package that outlasted its estimate, and a
policy run held by performance pacing at stage S4, whose wall clock keeps running while it is
held, after the blackout table changed following its admission (a newly activated catalog
event or a changed host entry). A stopped ladder ends and does not continue; a stopped startup
package goes back to its queue and runs after the blackout; a stopped policy run settles as a
condition termination and is rerun after the blackout (`policy-suspension.md`, "Condition
termination and owed reruns"). Client runs are never stopped.

The stop uses the checkpoint mechanism of the operator pause drain (`scheduling-pause.md`) with
its own reason; it is not an operator pause. The run's cancellation reason is the new
`ContainedTaskCancellationReason::BlackoutStop`, and `from_failure_code` maps
`blackout_run_stopped` to it (together with Workflow #385's `PerformanceYield` for
`perf_run_yielded` and `perf_hold_expired`). Both values are one revision of the interface
between the Runtime and its clients, so the Runtime and the UI ship it as a pair. Until SR-4b,
rules 1 and 5 of "Gradual quiet" still keep policy runs out of blackouts.

## Order of the deferrals

The policy evaluation decides a candidate's deferral by the first of these that applies; its
code is the decision's reason:

1. operator scheduling pause (`dispatch_paused_global`, `dispatch_paused_instance`);
2. the instance is held or in takeover cooldown (`dispatch_instance_held`,
   `dispatch_instance_cooldown`);
3. the task package is suspended (`pack_admission_suspended`, `policy-suspension.md`);
4. a blackout is active (`blackout_active`; SR-4a2: judged once per instance as the cycle
   starts, wake at the blackout's end);
5. the task package is not loaded (`pack_registry_not_loaded`, Workflow #389), then the pack
   schedule closes it (`pack_schedule_closed`, `instance-fact-store.md`, "Pack schedule facts");
6. performance pacing pauses the instance or its run interval has not passed
   (`perf_scheduling_paused`, `perf_pacing_gap`, Workflow #385), an owed rerun waits for the
   pacing stage (`policy_rerun_waiting`), or an entry-cost task waits for it
   (`policy_cost_commit_waiting`, `cost-commit.md`);
7. a blackout is approaching (`blackout_approaching`, "Gradual quiet"), then, on an instance
   whose `scheduling.day_boundary.quiet` is true in effect, the run does not fit its activity
   window by its expected duration (`policy_day_boundary_approaching`, "Day-boundary fit");

**8a.** a remedy round holds the task (`policy_remedy_waiting`) or stopped it for a person
(`policy_remedy_unchosen`, `policy_remedy_unavailable`, `policy_remedy_exhausted`; Workflow #411
B15, [remedies.md](remedies.md); written ahead of slice R4);

8. availability: budgets, activity window, interval, backoff, task pause (as today);
9. the random delay (`dispatch_random_delay_waiting`; Workflow #405 D1): on an instance whose
   `scheduling.random_delay.enabled` is true in effect (its own
   `instance.<id>.scheduling.random_delay.enabled` or the section's), a (task, instance) pair that
   passed positions 1 to 8 and has no draw draws one: `due_at` is the evaluation time, the delay
   is `min_ms` plus a value uniform in `0..=max_ms - min_ms` from a SHA-256 seed over the cycle's
   seed, the task, the instance alias and `due_at`, cut to the earliest of the intent's
   `facts_fresh_until_unix_ms`, the last millisecond of its activity window and (SR-4a2) the
   quiet point, the latest start that still passes position 7: the next blackout start minus the
   horizon and `quiet_margin_ms` (or `hard_quiet_ms` + 1 ms, the longer) and, where the
   day-boundary fit applies, the window's end minus the expected duration E (0 when nothing is
   left), and the planned start is `due_at` plus the delay. The pair is deferred with a wake at
   its planned start until then, and is dispatched at the first evaluation at or after it that
   lets it through; its `policy.dispatch_intent` carries `random_delay` (register X14). Every
   due pair of the instance draws its own delay, and a waiting pair does not take the instance,
   so another pair whose delay ended runs meanwhile. The draw is kept while an earlier position
   defers the pair and through a refused admission; it ends when the pair's latest admitted
   dispatch is at or after its `due_at`, when an evaluation leaves the pair not eligible (trigger
   false or unknown, feedback stop, placement block), when an evaluation passes the end of its
   cut, when the instance's delay is off (its waiting pairs are released at the next evaluation)
   and when the active catalog generation changes. Draws live in memory only: after a restart a
   pair draws again. Every policy dispatch waits, dependency follow-ups, remedy requests and
   their retries, immediate failure retries and fact refresh reads included; direct runs, Lab
   operations, ladders, startup packages and emulator autostart do not pass through this
   evaluation.

**Admission** (SR-4a2) judges positions 4 and 7 again, at its own time, under the admission
lock, after the intent's own checks (catalog, shape, ledger position, reason chain, approvals,
fencing, budget) and before the ceiling of its facts' freshness (`policy_facts_stale`) and the
remedy check of position 8a: an intent that crossed into a blackout, into the quiet before one
or past its day boundary since its evaluation is refused with the code the evaluation would
have deferred it with (`blackout_active`, `blackout_approaching`,
`policy_day_boundary_approaching`). The freshness ceiling comes after it because a catalog
`maintenance` event is a timeline boundary: in a V2 catalog it bounds the
`facts_fresh_until_unix_ms` of every intent evaluated before it, so an intent that crossed into
the event would otherwise always read `policy_facts_stale`, and the table of "Complete silence"
(the same code for every source) would not hold for catalog blackouts. The refusal is at Info: the
`policy.dispatch_rejected` record carries the code's outcome (its `instance_id` the instance's
opaque identifier) and the wake as `next_eligible_unix_ms`; no lease is taken and nothing runs.

Ladders and startup packages do not pass through this evaluation; their own entry points apply
the table of "Complete silence". Recognition-reading demands (Workflow #384) are collected only
from candidates that passed positions 1 to 7: an instance a blackout covers as the cycle starts
is held for the fact refresh as positions 1-2 hold one (SR-4a2), so its candidates carry
`blackout_active` and raise no reading demand. `actingctl schedule` and MCP `ac_schedule`
(ScheduleExplanation) show `blackout_active`, `blackout_approaching` and
`policy_day_boundary_approaching` as a candidate's `why`, with their values, and the wake as its
`next_wake_unix_ms`. For the clock-slot planning records of Workflow
#372 K2, `blackout_active`, `blackout_approaching`, `policy_day_boundary_approaching`,
`pack_schedule_closed`,
`perf_scheduling_paused`, `perf_pacing_gap`, `policy_rerun_waiting`,
`policy_cost_commit_waiting`, `policy_remedy_waiting` and `dispatch_random_delay_waiting` are
waiting gates (a slot is recorded as `clock_slot_deferred` only once its window has passed); `pack_admission_suspended`,
`feature_locked`, `policy_remedy_unchosen`, `policy_remedy_unavailable` and
`policy_remedy_exhausted` are persistent gates (`clock_slot_refused`). Position 8a's deferrals
carry no wake of their own: a remedy round's events, a changed choice or a lift wake the
evaluation.

## Owed occurrences and the bookkeeping day

A clock occurrence that falls inside a blackout stays owed (`scheduling/README.md`, "Clock
Sources") and is dispatched once at the first admissible evaluation after the blackout, against
the same bookkeeping day's budget. A condition rerun expires at the end of its bookkeeping
window (`policy-suspension.md`). Blackouts around a reset keep any run from crossing the reset.

## Configuration

| Key | Default | Meaning |
|---|---|---|
| `scheduling.blackout.host` | `[]` | Host blackout entries ("Configuration blackouts") |
| `instance.<instance id>.scheduling.blackout` (SR-4a1) | none (no entries of its own) | The instance's own entries ("Configuration blackouts") |
| `scheduling.blackout.quiet_margin_ms` | 60,000 | Margin of rule 1 and of each rung and startup package |
| `scheduling.blackout.hard_quiet_ms` | 0 | Rule 4 |
| `scheduling.blackout.stop_in_flight_at_start` | true | The safety net |
| `scheduling.blackout.quiet_horizon` (SR-4a1) | `deadline` | Rule 1's horizon: `deadline` or `expected` |
| `scheduling.day_boundary.quiet` (SR-4a1) | true | The day-boundary fit ("Day-boundary fit"); also per instance, `instance.<instance id>.scheduling.day_boundary.quiet`, default the section's |

The defaults live in the defaults file `distribution/windows/actingd.defaults.json` of the
configuration mechanism (Workflow #387) and arrive with the code that parses the keys (slice
SR-1 parses every `scheduling.blackout.*` key in v0.12.0). check-config lists each key with its
effective value and source in `config_manifest.parameters` from SR-1 on, but the keys take effect
only from SR-4a, so their presence says nothing about blackout support. SR-4a2 adds the
subsystem `scheduling_blackout` to `config_manifest.subsystems` (`enabled` true; the reason
`config_subsystem_always_on` carries as `count` the number of host blackout entries) in the same
PR as the policy's checks. A catalog generator detects
blackout support by that subsystem entry and refuses to emit maintenance blackouts without it,
because an older Runtime would ignore them silently. A key the key table marks settable can
also be changed while the Runtime runs (Workflow #387,
`ConfigureSettings`); the others are read-only there and change through the configuration file
and a restart.

## Ledger

The ledger is written only by the probes on the paths above; this document adds no direct
write. A dropped ladder is the existing `runtime.lifecycle_observed` suppression record
(`recovery_ladder_suppressed`) with the new reason `blackout`; `RecoveryLadderSuppression` gains
that persisted value, and builds from v0.12.0 on read it (registered by Workflow #383 AO-0 with
the other v0.12.0 shapes). The evaluation's deferrals are not ledger records, apart from the
clock-slot records of #372 K2; a startup package's wait shows only in the status. A stopped run
keeps its failure terminal with `blackout_run_stopped`.

## Codes

| Code | Category | Values | Slice |
|---|---|---|---|
| `blackout_active` | info | `instance_id`, `blackout_id`, `blackout_source`, `next_eligible_unix_ms` | SR-4a2 |
| `blackout_approaching` | info | `instance_id`, `blackout_id`, `blackout_source`, `blackout_start_unix_ms`, `required_ms` | SR-4a2 |
| `policy_day_boundary_approaching` | info | `instance_id`, `window_end_unix_ms`, `required_ms`, `next_eligible_unix_ms` | SR-4a2 (new; SCH-M D-6) |
| `blackout_run_stopped` | error | `blackout_id`, `blackout_start_unix_ms` | SR-4b |
| `blackout_config_invalid` | error | `field`, `blackout_source`; `blackout_id` when known | SR-1, SR-4a1; SR-4a3 (a catalog `maintenance` event without V2 validity, `blackout_source` `catalog`) |

`blackout_source` (vocabulary `blackout_source`: `host`, `instance`, `catalog`; SR-4a1) names
the list a refused entry is in, or the source of the entry a judgement reports.

None of them triggers the recovery ladder; the default ladder configuration contains none of
them (Workflow #387).
