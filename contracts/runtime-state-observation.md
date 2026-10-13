# Runtime state observations

`Status`, `MonitorStatus`, `ProjectInterface` and `DiscoverInstances` each commit one
bounded typed `RuntimeStateFact::Observed` in the original request's `command.validated` event.
The Runtime source/module/actor and request/correlation links identify the owner.
The response is derived from that committed event. Its `source` gives the EventId,
sequence and sampling start/end Unix milliseconds; the sampled status carries
the owner epoch. The existing receipt terminal convention remains unchanged.
Ledger sequences are gap-free from 1, so the online ledger event count equals
`status.source.sequence`; `repair_count` belongs only to the legacy segment backend
and is not applicable to the SQLite ledger (it is null offline as well).

The owner samples current scheduler/registry state using its existing locks.
The interval covers those reads; it does not assert a globally atomic instant
across different locks. At most 1024 instances are observed, and existing native
event/transport size limits still apply. A missing, invalid or uncommitted
observation is an explicit error. There is no added polling, state cache or
permission owner; Scheduler continues to decide lease/admission authority.

`DiscoverInstances` commits `RuntimeObservedState::InstanceDiscovery { owner_epoch,
provider_version, instances }`: the provider's discovery answer, each instance with its
`bound_alias` read under the registry lock after the vendor tool answered, ordered by index.
The variant has no `source` field; the response `RuntimeInstanceDiscovery` adds the committed
one. The interval covers the vendor tool run. The event carries the same Internal sensitivity
as the other state observations; a provider refusal commits no observation
(`emulator-control.md`, "Instance discovery query").

ProjectInterface preserves historical pagination position L for its catalog,
facts, decisions, approvals and diagnostics. The separately sampled current view
has committed position C in `observed_ledger_position` and `source.sequence`.
Legacy project responses carry the same source under `runtime.source`; L remains
the historical `runtime.ledger_position`. Each page has its own current observation
while retaining the original historical pagination cursor.

## Monitor state

Configuration and clear commands validate a proposed update under the existing
monitor lock. Their original `command.validated` event contains the configuration
version, prior/resulting registry revision, whether the update applied, and the
complete resulting instance status. Only then is the in-memory projection updated;
the command response is read from the committed payload. Idempotent commands keep
the configuration version and state while retaining their ordinary command receipt.

`monitor.completed` and `monitor.failed` retain the actual observation/decision
or failure and include the probe's configuration version, actual completed state,
and applied/current status. A stale probe is recorded with `applied: false`; it
cannot overwrite a replacement configuration or a clear/configure cycle. Recovery
replays these committed facts and never infers a running probe from an old start.

Before the listener and scheduling are enabled, the existing Host startup owner
opens MonitorRegistry with its already established GlobalLedger and owner epoch.
The original bounded journal validator reads `monitor.journal` once, up to 4 MiB.
One import event records the original SHA256, length, final revision and full final
state. Its marker prevents another import. A missing file is an explicit empty
baseline; an existing valid empty file retains its actual zero-byte source/hash.
The original final revision identifies the imported configurations. No historical
per-command events are manufactured.

The original file remains read-only history: there is no append or recovery
fallback to it. Recovery replays the ledger and checks that the retained import
source is unchanged. Corruption, a partial record, unknown instance, revision
conflict or changed source fails explicitly. A failed import preserves the original
file and records the cause through the existing Runtime failure payload when the
ledger remains writable; ledger failure propagates as fatal. The provider/startup,
resource-close, fencing and committed-effect owners retain their existing roles.

## Contained-run queries

`contained_run_status` reads a request or reverses a RunId through the same complete
semantic-fact identity used by Host recovery. A maintenance fact can be the run's
first fact before the main `PackageAdmitted`. Full and Brief project the same bounded
facts at one ledger snapshot; Brief omits step progress. Both retain the existing
page, event, resident-memory and time limits. A partial timed read stays incomplete.

A stored terminal has priority. Without one, a maintenance-only run in the current
epoch is `running`; a later `runtime.started` or `runtime.takeover` makes it
`interrupted_unterminated`. An actual main admission retains its admitted/running
states. The internal first-fact sequence drives interruption classification;
`admitted_sequence` and `restarted_after_admission` describe only the actual main
admission. Main package metadata remains absent until recorded, and maintenance
hashes belong only to `recovery_packages`.

`recent_runs` reads by run (Workflow #391 item 3): it first lists the runs of the
window by the facts each run records only a few of, its main admission
(`task.requested`), its entry facts (`task.entry_preflight`) and its terminal
(`task.completed`, `task.failed`, `task.cancelled`), one query per type, and never
reads the instance's other events. It deduplicates their request/run identities
before selecting the requested 1–10 runs, and orders them by their first fact,
newest first. A later fact in the time window does not make an older run recent.
The listing and each projection share one snapshot and the existing 20-second
budget; reaching that budget returns `incomplete`. MCP `ac_get_run`, `ac_diagnose`
and the latest-run lookup consume this projection directly. Beside the runs, `ac_diagnose`
carries at most 10 of the instance's `clock_slots` rows of `actingd suspended` recorded in its
window, refused and deferred rows before owed ones, newest first in each, with
`clock_slots_total` when fewer are kept; to fit its output budget it cuts them before any
other list but the two groups below (Workflow #372 K3, `policy-suspension.md`).

`actingctl diagnose` and `ac_diagnose` (Workflow #389 MCP-7) take the instance as optional:
without it they diagnose every instance, with the 10 most recent runs from the first page of
`list_contained_runs`. Both add `runs_grouped`, the window's failed and open runs from
`list_contained_runs` grouped by (policy task id `catalog_task_id`, failure code, final page),
and `failures_grouped`, the window's `runtime.failed`, `command.rejected` and the
`policy.execution_recorded` records of runs that paused their task, grouped by (type,
`catalog_task_id`, code); a code is its record's outcome, with an unregistered relayed
spelling read back (`scheduled_failure_code`). To fit its output budget `ac_diagnose` cuts
`failures_grouped`, then `runs_grouped`, the fewest-count groups first, before any other
list.

`list_contained_runs` (Workflow #389 MCP-3) pages the same listing for `actingctl runs`
and MCP `ac_runs`: the runs first recorded in a window, on one instance or all, newest
first by their first listing fact, each read in brief mode and kept by its dispatch and
state, at most 100 a page. As in `recent_runs`, a run is not listed until it writes its
first listing fact, which for a run with an entry check is when that check ends, at most
`min(step_timeout, task_timeout)`; meanwhile its instance shows `lease_active: true`.
A scheduled run also carries its policy task id as
`catalog_task_id`: the `task_id` of its `policy.dispatch_intent`. That event is linked to
the run but recorded under the admission request, not the run's own request, so it is
read with one query by run. A manual run has none. `task_prefix` matches
`catalog_task_id`, not the run's own `task_id`, so a manual run never matches it
(Workflow #389 MCP-3 ruling). A later page continues at the first page's snapshot, after
the last run the page before it examined. A listing the deadline ended early examines no
run; a page the deadline ended early is `incomplete` and continues after the last run it
read.
