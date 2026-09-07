# GlobalLedger query conditions

`EventQuery` is the shared, storage-independent query declaration. Ledger
`EventIndexes` selects candidates and applies the same predicates to live
queries, read-only snapshots and subscription projection. Indexes are rebuilt
from the persisted events, including their `origin.module` and optional payload
diagnostic code. Events without a diagnostic code do not match a code filter.

All selected conditions are conjoined. `from_sequence` and `to_sequence` are
inclusive; `minimum_severity` is an inclusive lower bound. Source, module,
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
Its existing `--severity` remains exact equality: the shared minimum narrows
candidates, then the offline adapter retains only that severity. A page's
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
