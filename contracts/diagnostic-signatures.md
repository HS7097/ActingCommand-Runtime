# Diagnostic signature replay

The GlobalLedger owns the signature catalog and pure matcher. A definition has
an exact `signature_id`, positive `version`, `origin_module`, `diagnostic_code`,
`event_type` and inclusive `minimum_severity`. Optional lifecycle conditions use
the existing stage, operation, code and resource-close fields. Selected conditions
are conjoined. A missing diagnostic code does not satisfy the tuple. Once the
tuple matches, a missing selected lifecycle field produces a `missing_fields`
row. Several definitions may match one event. These results classify recorded
contexts; they do not establish a unique root cause.

`signature.registered` and `signature.retired` are the catalog authority.
Reconstruction applies those typed events in sequence through a caller's frozen
bound. Registration starts at version 1. Retire the exact active registration
before registering the next version of that ID. A registration reference carries
its EventId, sequence, ID and version. Invalid transitions make the catalog
incomplete. The catalog supports at most 64 distinct IDs, including retired IDs.

## Explicit Runtime operations

Lab's three signature commands require a running Runtime. Their requests must
have both Lab actor and Lab source and use the ordinary Runtime receipt lifecycle.
The Host serializes registration/retirement validation and append, reconstructing
the catalog from its own ledger. The explicit match request supplies a historical
input root and bound; the Host opens that input read-only, reconstructs its own
catalog through `--catalog-through`, computes the result, and appends
`signature.matched`. Its receipt retains the terminal EventId/sequence. Query and
watch calls do not append signature events.

```text
actinglab lab signatures register --signature-id close_session_unconfirmed --signature-version 1 --origin-module runtime --diagnostic-code runtime.diagnostic --event-type runtime.failed --minimum-severity error --lifecycle-stage runtime.lifecycle.session_close --lifecycle-operation close_execution_session --lifecycle-code capture_backend_close_failed --cause-phase resource_close --cause-source nemu_ipc --resource-kind provider_connection --resource-phase disconnect_call --quiescence unconfirmed
actinglab lab signatures register --signature-id close_kernel_unconfirmed --signature-version 1 --origin-module runtime --diagnostic-code runtime.diagnostic --event-type runtime.failed --minimum-severity error --lifecycle-stage runtime.lifecycle.session_close --lifecycle-operation close_execution_kernel --lifecycle-code capture_backend_close_failed --cause-phase resource_close --cause-source nemu_ipc --resource-kind provider_connection --resource-phase disconnect_call --quiescence unconfirmed
actinglab lab signatures match --input-state-root <historical-root> --input-through <sequence> --catalog-through <registration-ledger-sequence> --limit 64
actinglab lab signatures retire --signature-id <id> --signature-version <version> --registration-event-id <event-id> --registration-sequence <sequence>
```

All options take one value; unknown, duplicate and missing options fail before
connection. The two example definitions describe session and kernel closing
contexts with an unconfirmed provider disconnect. Registration is an explicit
operation on the current catalog ledger. Existing historical inputs are not
modified to contain definitions.

## B read-only replay

```text
actingledger --state-root <historical-root> signatures --through <sequence> --catalog-state-root <registered-ledger-root> --catalog-through <sequence> --limit 64
```

B opens both roots with the existing `GlobalLedger::open_read_only` and verified
artifact reader. It returns a derived report with both native storage snapshots
and the shared match page. It takes no writer lock, repairs nothing and records
no matching event. The existing evidence ZIP `replay` command retains its contract.

Each prefix contains events from sequence 1 through the requested inclusive bound,
up to 16,384 events and 32 MiB of canonical event JSON plus newlines. The matcher
builds its retained prefix and SHA-256 in one bounded traversal; the native B
snapshot reader retains its existing storage-read behavior. The hash covers the
UTF-8 domain header `actingcommand.signature-prefix.v1\n` followed by each native
`PersistedEvent` serialized by `serde_json`, then a newline. The result carries the
requested and observed bounds, event count, digest and completeness. Limits fail
explicitly; no truncated input is reported as complete.

Pages contain at most 64 rows, ordered by source sequence then signature ID.
Every row binds the source EventId/sequence and registration reference. Totals
describe the entire bounded scan, while `row_offset`, returned rows and
`next_cursor` describe delivery progress. Pass the returned cursor JSON as
`--cursor` with the same two frozen bounds. Both prefix identities are validated;
changed input or catalog prefixes reject continuation. Later events beyond the
bounds do not enter matching.

Unreadable paths return the native read failure. Empty or incomplete catalogs,
bad tails, incomplete input and invalid catalog transitions yield explicit gaps.
Missing selected fields retain their source and registration identities.
`evidence_complete` is true only for complete prefixes with no gaps or missing
fields; it is independent of whether all result pages have been delivered. A
complete zero match still carries both prefix identities and the active catalog
count. Lab and B preserve incomplete reports and exit nonzero. B's native snapshot
reports retain the original corrupt-tail location and hash.
