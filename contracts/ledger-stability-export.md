# Read-only stability evidence export

`actingledger --state-root <root> export --stability [--after <sequence>]
[--through <sequence>] [--limit <count>]` emits JSON with `command: "stability"`.
It accepts the existing ordered sequence range and 1–1024 raw-event page limit.
Other filters and export modes cannot be combined with this mode.

The input is the existing GlobalLedger read-only snapshot. Snapshot validation
retains its full source checks; the page limit bounds projection, not snapshot
I/O. Each DiagnosticJson reference in the page is read through
`artifact-store::read_projected_verified`. The command does not scan for
unreferenced artifacts, write files, acquire the writer lock, or repair sources.

Only `actingcommand.runtime.contained-task-stability-comparison.v1` is expanded.
Each row contains the original persisted event and artifact reference/hash,
plus its recorded task/run/action/step, previous and current frame identities,
region, comparison mode/parameters, result, before/after consecutive counts,
threshold and nullable terminal reason. The artifact's current frame and run
must match its reference and event links; task and action must match the event.
Separate created and verified events retain their own sequences. Values are
not deduplicated, recomputed or converted into an integrity verdict. ExactPixels
has no difference metric, and timestamps do not establish monotonic duration.

Unknown stability versions, invalid/missing fields, mismatched source links,
pending redaction, unavailable objects, invalid JSON and native verification
errors are explicit failures. Other recognized diagnostic schema families
produce no rows or raw content. Neither error messages nor unsupported schemas
echo artifact content. A diagnostic without a string schema is explicitly
unclassifiable. Required nullable fields must still be present.

Stability diagnostic objects have a 16 KiB declared and observed file-size
limit before the canonical reader allocates their bytes. This follows the
ArtifactStore immutable published-object model. It covers the current small
v1 record while bounding each decoded/projection object. Oversized diagnostics
fail explicitly even if their schema cannot be inspected. Existing snapshot
verification of other artifact kinds is unchanged. Concurrent external mutation
of a published artifact is not an atomic snapshot of artifact bytes; native hash
verification remains authoritative. This leaf introduces no artifact locking.

`scanned_event_count` counts raw events; `scanned_diagnostic_count` counts their
diagnostic references; `matched_count` counts recognized stability schema-family
references, including unsupported or invalid family members. Rows count emitted
valid projections. Counts apply only to the returned page. The interval is
`(after_sequence, through_sequence]`; omitted through freezes the readable latest
sequence. Reuse it and `next_after_sequence` for later pages, including empty
matching pages. The next cursor is the last raw event scanned.

`has_more` and `next_after_sequence` describe readable pagination, not unreadable
suffixes. `gaps` retains incomplete storage, bad tail and unavailable upper-bound
states. `window_complete` requires no pagination remainder and no gaps/failures.
Snapshot artifact-verification failures preserve the native bad tail and original
reference/code/operation; their `source_sequence` is null because the failed
event was not admitted by GlobalLedger. Page projection failures carry its
admitted source sequence. A report with gaps is written and flushed before the
CLI returns nonzero `stability_export_incomplete`; pagination alone is successful.
