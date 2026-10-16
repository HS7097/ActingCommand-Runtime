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
