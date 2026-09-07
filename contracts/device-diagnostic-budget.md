# Device diagnostic budget

The resident Host keeps one device diagnostic slot for its `owner_epoch`, created
by `RuntimeHost::start`. The default `device_diagnostic_mode` is `shadow`; actingd
configuration may state this value explicitly. RuntimeStarted/RuntimeTakeover
records the selected mode and fixed detail limit in `device_diagnostics`.

The existing `append_event_under_fact_gate` persists the original event first.
Its device InputFailed/CaptureFailed diagnostic and cleanup fields, and the
device detail fields carried by Runtime lifecycle failures, then enter the slot
under the same fact gate. Each source field counts as one supplemental detail:
the source EventId, sequence, module and typed field identify its original
position. This count describes source fields, not distinct error occurrences.
C1B9 occurrence and observation/dropped counts retain their own meanings.

The first 16 source fields emit additional RuntimeLifecycleObserved facts with
phase `device_diagnostic_detail`. Further fields only update the bounded slot.
At most two bounded DiagnosticDetail records, the actual first and last fields,
are retained with emitted/folded counts and accumulated sensitivity. Counters are
checked, and overflow is fatal. Each detail retains the existing M1 token/message
limits and sanitizer. Required events, causes, Input/Capture/Lease outcomes and
task terminals are persisted completely through their existing owners.

The Host emits one `device_diagnostic_summary` after its final close lifecycle
facts and before ledger.close. An empty epoch emits a zero-count summary. Startup
failure after the configuration fact, and writable early close exits, also seal
the slot. A successful seal is cached; supplemental events never recursively
create supplemental events. Writer or supplemental append failure uses the
existing lifecycle failure flag and fatal path. Incomplete sealing returns an
explicit incomplete-summary status on the original error, retaining its code,
causes, native detail and resource disposition. The fatal last-word display also
identifies the incomplete summary and its failed operation. Missing or failed
sealing does not establish complete folded totals.

The Runtime payload contains the epoch, configuration, counts and first/last
source references. Full authorized projections retain the diagnostic details;
public projections remove both detail objects and retain their references and
counts. Sensitivity is the maximum observed declaration, including folded fields.
Consumers can query RuntimeLifecycleObserved in the existing online or offline
event entry and match each source sequence and EventId against the original fact.
No supplementary file, index, registry or background worker is a fact source.

Shadow retains existing private output. Facts committed by existing critical
transactions outside the named append hook remain complete original ledger
facts; this slot does not count them unless a later lifecycle append carries
their detail. Official shadow comparison, coverage, performance measurements
and a named cutover window remain pending. This source package grants no device
window or bypass switch.
