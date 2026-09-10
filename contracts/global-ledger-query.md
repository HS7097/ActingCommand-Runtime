# GlobalLedger query conditions

The private persistence boundary and staged database behavior are specified in
[GlobalLedger storage contract](ledger-store.md).

`EventQuery` is the shared, storage-independent query declaration. Ledger
`EventIndexes` selects candidates and applies the same predicates to live
queries, read-only snapshots and subscription projection. Indexes are rebuilt
from the persisted events, including their `origin.module` and optional payload
diagnostic code. Events without a diagnostic code do not match a code filter.

All selected conditions are conjoined. `from_sequence` and `to_sequence` are
inclusive; `minimum_severity` and `maximum_severity` are inclusive bounds.
`from_timestamp_unix_ms` is inclusive and `to_timestamp_unix_ms` is exclusive.
An equal pair of time bounds selects no events. Inverted time or severity bounds
are rejected by the typed request. Source, module,
diagnostic code, event type and typed association IDs use exact equality.
`origin_module` and `diagnostic_code` use the existing schema-owned enums.
Omitted new fields retain unfiltered behavior and are omitted from the wire
encoding, preserving query cursor identity for requests without new conditions.
Changing a selected condition changes the query-bound pagination cursor.

`actinglab lab watch` exposes the query through these value flags:

- `--from-sequence`, `--to-sequence`, `--event-type`, `--minimum-severity`, `--source`;
- `--origin-module`, `--diagnostic-code`;
- `--instance-id`, `--request-id`, `--correlation-id`, `--causation-id`,
  `--task-id`, `--run-id`, `--lease-id`, `--frame-id`, `--action-id`, `--recognition-id`.

`--req` is an alias for the correlation condition; use it or `--correlation-id`
once. `--request-id` is the distinct request link. Missing values, duplicate
conditions, unknown options, invalid IDs and unknown enum values fail before
connecting to Runtime. Help and capabilities publish these options. The
response's `filter` contains the actual typed query.

One watch call still returns one bounded batch. `--after` is exclusive and
intersects the query's inclusive range. Existing `--wait-ms`, `--max-events`,
idle response and next-cursor semantics remain the subscription owner's contract.

```text
actinglab lab watch --origin-module capture --diagnostic-code capture.failed --after 0 --max-events 64
actinglab lab watch --req <correlation-id> --after <sequence> --wait-ms 1000
actingledger --state-root <root> events --origin-module capture --diagnostic-code capture.failed --severity error --after 0 --through <sequence> --limit 64
```

Offline `actingledger events` converts its existing module, diagnostic and
correlation filters into the same typed query and uses read-only indexed pages.
Its existing `--severity` remains exact equality by setting both shared severity
bounds to that value. A page's
continuation indicates another matching event within the frozen through bound;
higher-severity candidates do not count as exact matches. Existing offline
commands that do not support event filters continue to reject them.

Queries read the original GlobalLedger facts. The offline leaf neither opens a
writer nor records matching facts, and creates no secondary signature store.

[Diagnostic signatures](diagnostic-signatures.md) provide explicit Runtime
registration, matching and retirement plus B's read-only historical replay.

`device-test ledger` is an independent read-only consumer of the same
`ledger-forensics` events request and result:

```text
device-test ledger --state-root <runtime-state> --origin-module capture --diagnostic-code capture.failed --severity error --after 0 --through <sequence> --limit 64
```

`--state-root` selects the Runtime state directory containing `ledger` and its
artifact store. Optional `--origin-module`, `--diagnostic-code`, `--severity`,
`--correlation-id`, `--after`, `--through` and `--limit` have the native offline
events semantics. Defaults are after zero, a snapshot-frozen upper bound, and
the leaf's maximum of 1024 events. Continue with the returned
`next_after_sequence` and the same `through_sequence` and filters.

The command emits the complete native JSON report, retaining event provenance,
verified artifact references and the leaf's sensitive-data projection. Native
read or validation failures propagate to the CLI's fatal exit. Dispatch occurs
before device configuration, ADB resolution or backend creation; device options
and commands cannot be combined with this read. Existing non-production device
commands retain their permissions and behavior. The selected facts come from
Runtime's GlobalLedger; the tool's execution journal is not imported as Runtime
facts.

## Shared view and snapshot types

`LedgerView::definition()` is the closed classification source for event stream,
observation, changes, errors, health and Lab. Snapshot page projection populates
`ProjectedEvent.views` with overlapping memberships. An omitted membership list
means that the projection has no snapshot membership context. The storage owner
derives SQL predicates from these
typed definitions; adding these interfaces does not install SQL views.

The errors view selects Warning, Error and Fatal without changing the original
severity. Observation and changes use the explicit families and event types in
the definition. Health selects the performance and monitor families. Missing
unload or clock-jump facts are not inferred from other lifecycle observations.

Lab includes direct Lab sources and LabRequest events. Related Runtime events
must share an existing Lab request/correlation anchor visible within the same
snapshot. Run expansion additionally requires an event that explicitly links
that run to the anchored request/correlation, also within that snapshot.

`RuntimeEventQueryPageRequest::at_snapshot` selects an existing position on the
first page. A continuation must match the query, profile and snapshot; the
fingerprint includes view, time and both severity bounds. Changing conditions
starts a new first page at the chosen snapshot. Changing a Runtime connection or
offline root starts a new query. A cursor carries no cross-source content identity.

`RuntimeEventQueryPage.read_scope` separates source completeness from `has_more`.
It gives the formal Runtime/offline source, material read state, actual
`scanned_through_position` and event-count, response-byte or incomplete-source
limits. A lookahead may make the read-through position greater than the last
returned row. The next cursor remains at the last returned row so byte trimming
cannot skip facts. An empty match can exhaust the requested snapshot even when
the source itself is incomplete. The existing 256-row and 768-KiB response limits
remain in force, and a single oversized result is rejected.

The Host delegates an event page to one `GlobalLedger::project_view_page` call,
so the writer's read branch freezes the position and derives the page together.
Subscriptions retain their receive limits and resume cursor. A Lab-filtered
subscription resolves the candidate against the ledger at that event's sequence;
run recovery groups are available through the fixed-snapshot page operation.

`run_recovery` contains derived groups, not changes to persisted events. Each
group includes the run, state, gaps and failure/success event IDs and positions.
Context comes from the complete related run through the snapshot, independently
of the selected page, module, time or severity filters. Context is bounded by the
existing 1024-event query ceiling per run; a larger run yields Unknown with an
explicit context limit. Missing relations, conflicting outcomes and incomplete
sources also yield Unknown.

Supported positive relations are input failure/completion with the same action,
recognition failure/later PageMatched with the same recognition/action links,
and native entry-recovery failure/completion with the same run and package.
An unrelated task completion does not resolve a failure. Profiles retain their
existing sanitized payload and artifact-key behavior; sensitivity and actor
remain provenance. The page projection reads no material bytes. Material access
and evidence of authorized eviction belong to the material reader.

## Offline view entry

`actingledger --state-root <root> views` uses `GlobalLedger::open_metadata` and
`GlobalLedgerMetadata::project_view_page`. `LedgerEventMetadata` contains the
validated event fields, typed payload and original artifact references. Source
validation verifies the ledger structure and integrity before exposing those
fields; material state remains `not_requested`.

The executable delegates typed parsing and reading to `ledger-forensics`:

- `--query <EventQuery JSON>` selects any combination of the shared conditions;
- `--profile <profile>` uses the existing projection profiles and defaults to `ui`;
- `--snapshot <position>` selects a first-page position or agrees with the cursor;
- `--cursor <next_cursor JSON>` continues the same query, profile and snapshot;
- `--limit <count>` uses the existing page default and maximum.

For example, the query object for exact Error events in one time interval is:

```json
{"view":"errors","minimum_severity":"error","maximum_severity":"error","from_timestamp_unix_ms":1000,"to_timestamp_unix_ms":2000}
```

Query and cursor arguments are bounded at 16 KiB and 2 KiB respectively.
Malformed, duplicate or unknown options fail before opening the root. The result
is the shared page under `command: views`. Source-incomplete pages are printed
with their explicit scope and then return a nonzero CLI exit; ordinary pagination
alone is not an error. The library entry is `run_views(ForensicViewRequest)`;
callers that already own the metadata snapshot can use `query_view_page`.

Changing the offline root starts a new query. Material verification for existing
chain, export and recovery operations retains its original meaning.
