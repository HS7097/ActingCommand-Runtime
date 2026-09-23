# Device diagnostic budget

The resident Host keeps one device diagnostic slot for its `owner_epoch`, created
by `RuntimeHost::start`. The default `device_diagnostic_mode` is `shadow`; actingd
configuration may state this value explicitly. RuntimeStarted/RuntimeTakeover
records the selected mode and the detail limit in `device_diagnostics`; the limit
is 0 because no per-occurrence detail fact is written (ledgers written before
Workflow #328 record 16).

The existing `append_event_under_fact_gate` persists the original event first.
Its device InputFailed/CaptureFailed diagnostic and cleanup fields, and the
device detail fields carried by Runtime lifecycle failures, then enter the slot
under the same fact gate. Each source field counts as one detail: the source
EventId, sequence, module and typed field identify its original position. This
count describes source fields, not distinct error occurrences. C1B9 occurrence
and observation/dropped counts retain their own meanings.

The primary failure event, carrying its complete detail, is the only
per-occurrence fact. Since Workflow #328 a source field only updates the slot in
memory; the hook writes nothing to the ledger. The slot retains two bounded
DiagnosticDetail records, the actual first and last fields, with the count of
all fields in `emitted_count` (no cap), `folded_count` 0, and accumulated
sensitivity. The counter is checked, and overflow is fatal. Each detail retains
the existing M1 token/message limits and sanitizer. Required events, causes,
Input/Capture/Lease outcomes and task terminals are persisted completely through
their existing owners.

Ledgers written before Workflow #328 also hold up to 16 RuntimeLifecycleObserved
facts per epoch with phase `device_diagnostic_detail`, one for each of the first
16 fields, and their summaries count those in `emitted_count` and further fields
in `folded_count`. These records still decode and validate; no reader depends on
them.

The Host emits one `device_diagnostic_summary` after its final close lifecycle
facts and before ledger.close. An empty epoch emits a zero-count summary. Startup
failure after the configuration fact, and writable early close exits, also seal
the slot. A successful seal is cached; the summary never re-enters the
observation hook. Writer or summary append failure uses the existing lifecycle
failure flag and fatal path. Incomplete sealing returns an explicit
incomplete-summary status on the original error, retaining its code, causes,
native detail and resource disposition. The fatal last-word display also
identifies the incomplete summary and its failed operation. Missing or failed
sealing does not establish complete counts.

The Runtime payload contains the epoch, configuration, counts and first/last
source references. Full authorized projections retain the diagnostic details;
public projections remove both detail objects and retain their references and
counts. Sensitivity is the maximum declaration across all counted fields.
Consumers can query RuntimeLifecycleObserved in the existing online or offline
event entry and match each source sequence and EventId against the original fact.
No supplementary file, index, registry or background worker is a fact source.

Shadow retains existing private output. Facts committed by existing critical
transactions outside the named append hook remain complete original ledger
facts; this slot does not count them unless a later lifecycle append carries
their detail. In particular InputFailed committed through `execute_critical`
does not pass the hook and is not counted. Per-occurrence detail facts were
retired rather than cut over, so no shadow comparison or cutover window is
pending for them. This source package grants no device window or bypass switch.
