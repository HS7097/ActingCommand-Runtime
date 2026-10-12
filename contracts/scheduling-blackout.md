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
anything reads them; they take effect only from SR-4a. Until SR-4a lands, timeline
`maintenance` events and host blackouts have no Runtime consumer: a catalog generator must not
rely on blackouts before check-config reports the `scheduling_blackout` subsystem that only
SR-4a adds ("Configuration").

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

### Host blackouts

A host blackout is an entry of the actingd configuration list `scheduling.blackout.host`; it
covers every instance of the host. One entry is
`{id, weekdays, utc_offset_minutes, start_minute_of_day, end_minute_of_day}`, the time fields
having the shape and bounds of an activity window (`scheduling/v2/activity.schema.json`,
`window`): `weekdays` 1–7 (unique, at least one), `utc_offset_minutes` within ±840,
both minutes within 0–1439, an `end` at or before `start` crossing midnight. `id` is unique
in the list. An entry that breaks a rule fails check-config and startup with
`blackout_config_invalid` (`field` points at the member). The default list is empty; an
installation adds entries for its own needs, for example its planned machine restarts, or a
night period. Entries have no `applies_to`: every blackout is complete silence.

### Activity profiles become bookkeeping days

With blackouts in place, an activity profile no longer says when an instance may run. Each
instance has one daily window `[reset, reset − 1 min)`, so that its day is the game's day
(an overnight window's `local_day` is the day it opens). A window with `start == end` is not
used for this, because it always starts at local midnight. Without a blackout, an instance may
run at any time its budgets allow. The generator change that rewrites the activity profiles is
data-side (slice SR-8), not Runtime code.

## Pure judgement

The policy crate holds two pure functions next to `activity_window_at`:
`blackout_at(blackouts, instance, t)` returns the blackout covering the instance at `t`
(`{id, start, end}`) or none, and `next_blackout_start(blackouts, instance, t, bound)` returns
the next start within the bound or none. Both read only the active catalog's maintenance
events and the configured host entries.

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
| Emulator autostart (`autostart` claim, also after a watchdog start) | Held: no key, no emulator start; it stays queued with a bounded wait | Starts the emulator; the startup package follows |
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

## Gradual quiet

1. Every policy admission requires `now + horizon + quiet_margin_ms ≤ next blackout start`,
   checked in the evaluation and again at admission. `horizon` is the run's response deadline,
   the wall-clock hard limit that also sets its lease time. It is not multiplied by the
   performance pacing factor, because pacing never lengthens the wall-clock limit (Workflow
   #385). The next start is searched only up to `now + horizon + quiet_margin_ms`.
2. A candidate that fails it is deferred with `blackout_approaching` and wakes at the end of
   that blackout. Shorter candidates still run, so the closer the start, the shorter the work
   that is admitted: there is no fixed quiet start.
3. A startup package is taken out of its queue, and a ladder starts each rung, only under the
   same condition with its own time limit; otherwise it waits (startup package) or ends
   (ladder) as above.
4. From `start − hard_quiet_ms` (default 0, off) no new policy dispatch, ladder rung or startup
   package starts.
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
4. a blackout is active (`blackout_active`);
5. the task package is not loaded (`pack_registry_not_loaded`, Workflow #389), then the pack
   schedule closes it (`pack_schedule_closed`, `instance-fact-store.md`, "Pack schedule facts");
6. performance pacing pauses the instance or its run interval has not passed
   (`perf_scheduling_paused`, `perf_pacing_gap`, Workflow #385), an owed rerun waits for the
   pacing stage (`policy_rerun_waiting`), or an entry-cost task waits for it
   (`policy_cost_commit_waiting`, `cost-commit.md`);
7. a blackout is approaching (`blackout_approaching`);
8. availability: budgets, activity window, interval, backoff, task pause (as today).

Ladders and startup packages do not pass through this evaluation; their own entry points apply
the table of "Complete silence". Recognition-reading demands (Workflow #384) are collected only
from candidates that passed positions 1 to 7. For the clock-slot planning records of Workflow
#372 K2, `blackout_active`, `blackout_approaching`, `pack_schedule_closed`,
`perf_scheduling_paused`, `perf_pacing_gap`, `policy_rerun_waiting` and
`policy_cost_commit_waiting` are waiting gates (a slot is recorded as `clock_slot_deferred` only
once its window has passed); `pack_admission_suspended` and `feature_locked` are persistent
gates (`clock_slot_refused`).

## Owed occurrences and the bookkeeping day

A clock occurrence that falls inside a blackout stays owed (`scheduling/README.md`, "Clock
Sources") and is dispatched once at the first admissible evaluation after the blackout, against
the same bookkeeping day's budget. A condition rerun expires at the end of its bookkeeping
window (`policy-suspension.md`). Blackouts around a reset keep any run from crossing the reset.

## Configuration

| Key | Default | Meaning |
|---|---|---|
| `scheduling.blackout.host` | `[]` | Host blackout entries ("Host blackouts") |
| `scheduling.blackout.quiet_margin_ms` | 60,000 | Margin of rule 1 and of each rung and startup package |
| `scheduling.blackout.hard_quiet_ms` | 0 | Rule 4 |
| `scheduling.blackout.stop_in_flight_at_start` | true | The safety net |

The defaults live in the defaults file `distribution/windows/actingd.defaults.json` of the
configuration mechanism (Workflow #387) and arrive with the code that parses the keys (slice
SR-1 parses every `scheduling.blackout.*` key in v0.12.0). check-config lists each key with its
effective value and source in `config_manifest.parameters` from SR-1 on, but the keys take effect
only from SR-4a, so their presence says nothing about blackout support. SR-4a adds the subsystem
`scheduling_blackout` to `config_manifest.subsystems` (`enabled` true; the reason carries the
number of host blackout entries) in the same PR as the checks. A catalog generator detects
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
| `blackout_active` | info | `instance_id`, `blackout_id`, `next_eligible_unix_ms` | SR-4a |
| `blackout_approaching` | info | `instance_id`, `blackout_id`, `blackout_start_unix_ms`, `required_ms` | SR-4a |
| `blackout_run_stopped` | error | `blackout_id`, `blackout_start_unix_ms` | SR-4b |
| `blackout_config_invalid` | error | `field`; `blackout_id` when known | SR-4a |

None of them triggers the recovery ladder; the default ladder configuration contains none of
them (Workflow #387).
