# Scheduling Catalog V2

All four documents declare `actingcommand.scheduling.v2` and one identical catalog
descriptor. The same `compile_catalog` entrypoint applies the declared version's
field and reference rules and produces the existing `CompiledCatalog`. Unknown
versions, mixed versions, unknown fields, duplicate keys, dangling event IDs and
invalid ranges reject the complete catalog. The schemas in this directory define
V2; the sibling V1 schemas retain their own contract and serialized identity.

Instance scope and override `instance_id` fields use the exact registered alias,
as in V1: 1–256 UTF-8 bytes without Unicode control characters. Case, permitted
whitespace and Unicode are preserved. JSON Schema bounds character length and
the shared contract/compiler enforces the UTF-8 byte bound. Other identifiers,
references and the Runtime's typed `InstanceId` retain their existing rules.

Every V2 timeline event requires `validity` with `from_unix_ms` and
`until_unix_ms`. Both timestamps use Unix milliseconds; the lower bound is
inclusive and the upper bound exclusive. An explicit null upper bound means
unbounded. A finite upper bound must exceed the lower bound. All declaration
integers remain within the existing canonical safe-integer limits.

A V2 timeline event may carry `excludes_fact_prefixes` (Workflow #411 P19,
since 0.12.25; `../README.md`, "Reset exclusions and the task's own last
result"): the key prefixes it does not invalidate. A key that starts with one of
its `invalidates_fact_prefixes` and with none of these is invalidated as
before; an excluded key keeps its observation and expires by itself. At most
128 entries, each an identifier (`type_mismatch` at the entry) below one of the
event's `invalidates_fact_prefixes` (`dangling_reference` at the entry
otherwise). A V1 document that writes it is refused as an unknown field at the
member. An event without it serializes, and hashes, exactly as before.

The task predicate `{"kind":"timeline_active","event_id":"neutral.window"}`
references an event in the same catalog. The existing schedule evaluator resolves
the latest occurrence using its declared `ClockSource`. A positive duration gives
the occurrence interval `[occurrence, occurrence + duration_ms)`, intersected
with the event validity. The predicate is true only inside that intersection and
when the event scope matches the current Runtime instance. Overlapping recurring
occurrences use the latest occurrence. A local interval still uses the pinned
monotonic clock, projected to Unix time by the existing clock owner.

An applicable closed or expired interval is false. A zero-duration event or an
inapplicable scope is unknown and cannot become permission through `not`.
Missing references fail compilation; unrepresentable occurrence/end arithmetic
returns an explicit evaluation error. No partial evaluation grants dispatch.
The existing `all`, `any` and `not` combinators retain their Boolean semantics.
Instance windows must be authored only from effective instance facts; the
evaluator does not infer missing instances or create windows from unknown facts.

Future validity starts, scheduled starts, occurrence ends and validity ends
contribute to `next_wake_unix_ms`. Wakes may be conservative, including events
outside a particular task's scope. This also covers events beneath short-circuit
Boolean branches. V2 dispatch freshness is capped before the earliest timeline
boundary, using the existing inclusive admission freshness ceiling. Reevaluate
at the boundary; an intent evaluated before it cannot be admitted at it.

The canonical catalog hash includes the version, predicate references and full
event validity. Source documents retain their immutable generation identities;
`reveal_source` supplies the existing evidence reference. The compiler never
dereferences a URL. Evaluated timeline predicates add event ID, evaluation time,
active interval and next boundary to the existing decision reason chain.

Consumption remains `compile_catalog` -> Runtime catalog activation ->
`PolicyHost` -> `policy::evaluate` -> manifest binding and admission. A weekly
event at minute 240 with offset +480 and duration 86400000 expresses a 04:00 to
04:00 window. Date and scope values belong to the catalog. This contract covers
availability; daily activity budgets continue to follow their existing owner.

A `clock` predicate may carry `missed` (Workflow #361 R7), V2 only:
`{"kind":"skip_after","grace_ms":N}` with N from 60000 (one minute) to
604800000 (seven days). An unserved occurrence O of that clock is due only
before `O + grace_ms`; after it the occurrence is skipped and the clock waits
for its next occurrence (`../README.md`, "Clock Sources"). `missed` under a
`not` is refused (`predicate_uncomputable`), an unknown `kind` is a
`type_mismatch`, and a V1 document that names it is refused as an unknown
member. A clock without `missed` serializes, and hashes, exactly as before.

ROUT-L RL-1a ([../../standard-pack-schedule.md](../../standard-pack-schedule.md)): a V2 catalog
may carry an empty `approval_refs` and an empty task list (the standard-pack composition, which
has no approval, and a composition without rules); V1 keeps its rules. A V2 task, pool, activity
profile or timeline event id, and a reference to one, may also be the id `<identifier>@<alias>` of
an instance copy the composer makes (schema `itemId`: an identifier, `@`, and an instance alias
without whitespace or a control character, at most 128 bytes in all). One continued id may be
claimed by several tasks when each is scoped to a different instance (the copies of one rule);
any other second claim stays a split.

A task may carry `continues_from` (Workflow #372 K5), V2 only: 1 to 8 task
ids of an earlier catalog that this task continues, each by the rule of a task
id. Under the active catalog the task accounts as itself and these ids
(`../README.md`, the paragraph after "Task ids are global"), so a rename that
activates during a day keeps that day's budgets, slot, outcome and failure
state. A listed id that is a task of the same catalog, one listed twice in one
task, and one that two tasks claim (a split) are each `duplicate_id` at the
item; an empty list or more than 8 ids is `limit_exceeded` at the member; a V1
document that names it is refused as an unknown member. Nothing is checked
against the ledger, so an id that never ran adds nothing. A task without
`continues_from` serializes, and hashes, exactly as before.

A task may carry `dispatch_parameters` (Workflow #411 B8), V2 only: a boolean.
With `true`, every selected policy dispatch of the task, with or without
remedies, carries its schedule values on `policy_dispatch_parameters`
(`next_occurrence_unix_ms`, `window_end_unix_ms`, `last_in_window`,
`last_daily_slot`, `daily_remaining`, each when known), and its run reads them
as the read-only facts `session.dispatch.schedule.<key>`
(`../../remedies.md`, section 4). With `false` the task carries none, as
without the member. A V1 document that names it is refused as an unknown
member. A task without `dispatch_parameters` serializes, and hashes, exactly as
before.

A task may carry `reserves` (Workflow #411 B16 part 2), V2 only: 1 to 128
entries `{"pool_id": …, "at_least": N}`, the least amount, in the pool's real
units, that each named pool must still hold after a run of the task. The
evaluator does not admit a candidate that would leave a pool below it
(`resource_reserve_insufficient`, `../README.md`, "Task Reserves"). A pool the
catalog does not declare is `dangling_reference` and a pool named twice is
`duplicate_id`, each at the entry's `pool_id`; N above the pool's `capacity` (a
constant; #386 CF-5) is `limit_exceeded` at `at_least`; an empty list or more than 128 entries is
`limit_exceeded` at the member; a V1 document that names it is refused as an
unknown member. A task without `reserves` serializes, and hashes, exactly as
before.

SS-6 (since 0.12.31; register §1.218; `../../remedies.md`, section 4, "Spend
values"): a selected dispatch of a task whose `reserves` names a pool its
`consumes` lists, or whose `produces` lists a pool an active resource goal
covers, hands its run, without `dispatch_parameters`, per such `consumes` entry
`i` the allowance `session.dispatch.schedule.spend_allowance.<i>` (the reserve
gate's available amount, the pool's projection less the other tasks' consumption
its reading does not show yet, less the reserve, at least 0), per such
`produces` entry `j` the goal's gap `session.dispatch.schedule.gap_remaining.<j>`,
and `session.dispatch.schedule.run_cap` (at most 1000), which reads each
consumed pool's per-run `consumes[].amount` and, when no produced pool carries a
base weight, each produced pool's per-run `produces[].amount` (an
`expected_amount_milli` production with an open gap there gives no run cap).
`<i>` and `<j>` are positions in the task's own lists, from 0. No member is
added: a catalog serializes, and hashes, exactly as before.

A predicate may be `{"kind":"fact_time_reached","scope":…,"fact_key":…,"offset_ms":N,"max_age_ms":M}`
(Workflow #405), V2 only: true once the `timestamp_ms` value of the fact plus N
has come, false with its reason before that or when the fact gives no time
(`../README.md`, "Fact-time wake"). All five members are required. `scope` and
`fact_key` follow `fact`'s rules; N is a signed integer from -86400000 to
86400000 and M is null or from 1 to 31536000000, otherwise `limit_exceeded` at
the member. A V1 document that names the kind is refused as an unknown field at
the predicate's `kind`. A catalog without it serializes, and hashes, exactly as
before.

Two edge members (SCH-M CF-1), V2 only, each an optional boolean
(`../README.md`, "Condition edges"):
- `dependency_completed` may carry `since_own_run`. With `true` the predicate
  is an edge: true once per new accepted terminal of the predecessor, until
  this task has run successfully since the predecessor's latest dispatch
  (`dependency_already_served` while it has). With `false`, or without the
  member, it reads the predecessor's latest terminal state as before.
- `fact_time_reached` may carry `once_per_reading`. With `true` a reached time
  is served once: after this task's latest run succeeded and was dispatched at
  or after `T + N` and the reading's observation, the predicate is false
  (`fact_time_served`) until a newer reading. With `false`, or without the
  member, it is true from `T + N` on as before.

A V1 document that writes `since_own_run`, with either value, is refused as an
unknown field at the member; `fact_time_reached` is V2 only as a whole. A
catalog without either member serializes, and hashes, exactly as before.

A `fact` predicate's `value` may be a fact reference (#386 CF-4), V2 only:
`{"type": "fact", "fact_key": B, "offset": N}` (`../README.md`, "Fact
references"; schema `factReference`). The predicate then compares the integer
its fact holds with the integer the fact B of its own scope holds plus N; both
facts obey its `max_age_ms`, and a B that gives no integer makes it false with
`fact_reference_unavailable`. All three members are required. N is a signed
integer within the canonical integers, otherwise `limit_exceeded` at
`value/offset`; B follows `fact_key`'s rules (`type_mismatch` at
`value/fact_key`); `contains` is `predicate_uncomputable` at `comparison`; a
reference that names a `scope` is refused as an unknown field at `value/scope`
(it always reads the predicate's scope). A V1 document that writes one is
refused as an unknown field at `value/type`. A catalog whose `fact` predicates
hold typed values serializes, and hashes, exactly as before.

SS-5 (since 0.12.32; register §1.223; `../README.md`, "Self-read fact probe"):
a `fact` predicate may carry `on_unknown`, V2 only, whose one value is `probe`
(schema: `{"enum": ["probe"]}`). It names a fact only the task's own package
reads: while the fact is unknown the predicate is true once per unknown period
(`fact_probe`), so the task runs and its run reads the fact, and no fact refresh
is asked for the key; after a successful run of the task dispatched in that
period it is false (`fact_probe` with `served_by_dispatch_unix_ms`) until a
reading, a reset or an expiry. Its scope must be an instance (the only scope a
reading writes) and its `fact_key` must start with `resource.` or `inventory.`
(a reading mark's key), and it must stand in the `trigger`, reached through
`all` and `any`, never under `not` and never in the `feedback_stop`; otherwise
`predicate_uncomputable` at `on_unknown`. A V1 document that writes it is
refused as an unknown field at the member. A catalog without it serializes, and
hashes, exactly as before.

A pool's `capacity` and a `resource_projection`'s `value` may each be a fact
reference too (#386 CF-5), V2 only: `{"type": "fact", "fact_key": B, "offset":
N}` (`../README.md`, "Pool fact references"; schema `factReference`). Both read
B in the pool's own scope. A capacity is then, at each evaluation, the integer B
holds plus N (at least 1, or the pool has no value and its projection is Unknown
with B's reason); a reading above it stays as read and regeneration stops at
it. A comparand compares the projected amount with B plus N, and a B that gives
no integer makes the predicate false with `fact_reference_unavailable`. The
members follow the rules above, at `capacity/…` or `value/…`: N within the
canonical integers (`limit_exceeded` at `…/offset`), B by `fact_key`'s rules
(`type_mismatch` at `…/fact_key`), no `scope` (an unknown field at
`…/scope`); a V1 document that writes one is refused as an unknown field at
`capacity/type` or `value/type`. Against a capacity read from a fact, a
reserve's `at_least` and an effect's quantity are not checked at compilation.
A catalog whose capacities and projection values are numbers serializes, and
hashes, exactly as before.

SS-3 (since 0.12.30; register §1.212; `../README.md`, "Optional run-count
bounds"): a V2 task may leave out `loop_budget`, and each of its items
`daily_limit`, `window_iteration_limit` and `max_runtime_ms` on its own; a V2
activity profile may leave out each of `daily_budget`, `max_window_iterations`
and `session_max_ms`. A bound that is not written is no dimension: admission
reserves nothing for it, the retry round does not judge it, the budget receipt
carries none of its members, Workflow #411 B8 hands no `daily_remaining` or
`last_daily_slot` from it and the strategy estimate uses the bounds that
remain. A written bound must still be above 0 (`loop_budget_missing`) and
applies exactly as before; an explicit `null` is refused (`type_mismatch`), as
these schemas declare, and is never read as not written; a V1 document still
requires all six
(`missing_required_field` at the member). An absent bound serializes, and
hashes, as nothing at all, so a catalog that writes every bound serializes, and
hashes, exactly as before.

SS-2 (since 0.12.32; register §1.222; `../README.md`, "Ties and chain heads"):
no member is added; the order of `tasks` and the shape of a trigger now matter
to ranking, in V1 as in V2. A task whose trigger requires another task's success
(`dependency_completed` accepting `succeeded`, reached only through `all`)
ranks with that chain's head's `priority` and `strategic_weight_milli` while it
continues the head's latest successful run, and with its own otherwise. Of
candidates the ranking leaves equal, the one closer to losing its eligibility
(a required `timeline_active` or `missed` clock closing sooner, or fewer runs of
its declared `consumes` left above a required threshold or a `reserves` line)
goes first, then the one with more required conditions, then the one earlier in
`tasks`. A catalog serializes, and hashes, exactly as before.

SS-2b (since 0.12.32; register §1.225; `../README.md`, "Net benefit within a tier"), V2 only:
- A task may carry `benefit_judged`, a boolean. With `true` it is a business task: its
  candidate (unless it ranks with a chain head or is a manual request) is judged by its net
  benefit within its tier and is not dispatched while that benefit is unknown or not positive.
  With `false` it is as without the member. The standard-pack composer writes `true` on every
  rule whose package is not maintenance-role and that does not declare itself maintenance
  (SS-2c, since 0.12.32: the `schedule.json` rule member `"maintenance": true`, which is no V2
  task member and is `unknown_field` here), and refuses a rule that writes it itself. A V1
  document that names it is refused as an unknown field at the member.
- A pool's `valuation` may be the string `"pending"`, and an effect's `amount` (either
  direction, as the effect's one quantity) may be `"pending"`: the explicit marker that the
  value is not known yet (Alice's ruling 116: 「无害占位符或者空值」). A marker changes nothing but a
  judged task's net benefit, which it makes unknown. Any other string is `type_mismatch`; a V1
  document that writes a marker is refused as `type_mismatch` at the member; an `amount` marker
  beside `expected_amount_milli` is `limit_exceeded` at the entry.

A catalog without these serializes, and hashes, exactly as before.
