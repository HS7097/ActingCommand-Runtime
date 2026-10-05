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

`recent_runs` discovers semantic facts, deduplicates their request/run identities
before selecting the requested 1–10 runs, and orders them by their first fact,
newest first. A later fact in the time window does not make an older run recent.
The listing and each projection share one snapshot and the existing 20-second
budget; reaching that budget returns `incomplete`. MCP `ac_get_run`, `ac_diagnose`
and the latest-run lookup consume this projection directly.
