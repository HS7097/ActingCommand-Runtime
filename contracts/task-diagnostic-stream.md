# Task diagnostic stream

The existing task terminal may carry `task_timing`, a bounded observation of
`recognition_evaluate`, `diagnostic_record_write`, `capture_recognition` and
`recognition_completed_record`. Each has separate preflight,
execution and finalization counts, error counts, accumulated/maximum microseconds
and a last sample with its actual frame/recognition or record index. The clock is
the current process's `std::time::Instant`. These spans do not share the origin of
RuntimeClock step/dispatch values, summary `runtime_ms` or event Unix timestamps.
The recognition result describes the outer page-batch Result; individual page
failures remain in the original page outcomes. Home preflight does not measure
this batch call and remains unobserved; entry recovery retains its own budget
origin within the preflight phase.

The kernel supplies its original start/deadline only for observation. A last
sample records the task budget known before that call; absent/not-started budgets
and incomplete conversions are explicit. A measured zero is a valid measurement,
not a replacement for a missing sample. Checked count, duration or accumulation
failure marks the observation incomplete without changing the original result.
The original `TaskTimingFailure` remains in milliseconds and is also retained in
the terminal observation with its phase and budget origin.

The record-write span includes the original encode/framing/append call, including
an Err return, and excludes the document footer and seal. It never writes its own
measurement into that record. The existing task terminal receives the snapshot
after the last diagnostic record returns. An existing permitted RuntimeFailed
lifecycle record can carry the snapshot when diagnostics are aborted; the
original deduplication and LedgerFailure prohibition are unchanged. Missing
terminal/failure evidence, interrupted execution and older records do not supply
timing observations. No extra event or artifact write is introduced.

`capture_recognition` measures the original Host recognition-state handling and
CapturePipeline recognition call through its returned Result, including early
returns and errors. `recognition_completed_record` measures only the original
RecognitionCompleted arm, including its ordered recognition and task Ledger
commits; the preceding active check and trace offset are outside that span.
Both freeze the original frame/recognition identities before the call, record
the original Result with record_index absent, and update only the fixed in-memory
Observer summaries. Identity clearing still follows both successful commits.
Unentered summaries are omitted and decode as Unobserved; observed zero and
Incomplete remain explicit. The existing snapshot and terminal carry the last
completed calls without an extra event or query.

The optional task_failure.check_position is postcondition_before_capture or
postcondition_after_capture only for Task/Postcondition timing failures. It
identifies the original await_postcondition deadline check that returned the
error. A missing position is unobserved. The original scope, stage, elapsed/limit
values, checks and sleeps are unchanged; entry recovery forwards the same error
position while retaining its original observation phase and budget origin.

Each phase may also carry a fixed `boundaries` object. `capture_page` covers the
original kernel call from before capture through page/scene/OCR result handling,
including returned errors. `capture` covers the complete Host capture call;
`capture_active_pressure` covers its initial active and pressure checks,
`capture_backend` the original retained backend call, and `capture_material` the
successful backend arm through its returned frame or error (FrameStore, PNG,
material persistence, pinning, pressure and configuration handling).
`capture_completed_record` and `recognition_started_record` cover their original
Host arms. `input` covers the complete original Host input call. `post_input_wait`,
`retry_wait`, `page_recognition_wait` and `postcondition_wait` cover only their
original wait calls, with the original duration expressions and results.

`effect_completed_record` covers the complete original Kernel record call,
including Host active/trace handling and the EffectCompleted arm. Its original
Result and Task budgets are retained before error propagation. `input_to_effect_completed`
starts at the existing input observation's end, before its aggregation, and ends
at that record return. `effect_completed_to_post_input_wait` starts at the same
record return and ends at the original post_input_wait start; it includes the
original operation-state update and observation/entry preparation. The original
input and wait endpoints are unchanged. Two fixed Observer markers require the
same task, phase, timing context, step, action and known frame/recognition. They
are consumed once and cleared on a new input, context or phase. An unmatched or
unclosed marker is Incomplete in the existing snapshot; it never borrows a later
action's endpoint. A returned record error completes the first bridge as Err;
it does not start the second bridge.

`recognition_payload_append` and `recognition_task_append` retain the two ordered
RecognitionCompleted commits separately. `effect_completed_append` retains the
single original TaskEffectCompleted commit, before the unchanged capture-evidence
transition. Their fixed `recognition_payload_stages`, `recognition_task_stages`
and `effect_completed_stages` contain `fact_gate` (original lock acquisition),
`draft` (construction and sanitization), `writer_response` (the original Ledger
call), `device_diagnostics`, `fact_sync` (including required invalidation), and
`pipeline` (the original performance callback after dropping the Fact gate).
An original error completes the entered observations before it propagates;
later, unentered stages remain Unobserved.

The same Ledger append reply transports process-local endpoints for `ledger_queue`
(immediately before sending through writer receipt), `ledger_persistence` (writer
receipt through backend persist return, including validation/preparation), and
`ledger_publication` (successful backend return through retention/index/event/
statistics publication and original reply preparation). `ledger_durable` directly
covers only that backend persist call inside persistence. An earlier store error
ends persistence at store return; publication remains unentered. Channel creation,
sender setup, response send/wakeup tails and post-reply delivery are outside these
inner spans. No cumulative statistics subtraction supplies a missing endpoint.
Missing same-request reply observations stay absent; partial endpoints/results
remain Incomplete, with unknown duration/result represented explicitly.

`ledger_send` covers the same original send_command call, after command construction,
with its own original Result. Queue retains its original endpoints. Each append
stage family also retains one optional `writer` snapshot from that same reply,
replaced on every append; a missing reply never reuses the previous append's data.
It includes the send return and writer receipt relative to this send's start,
and the writer's immediately preceding completed command kind, processing span,
reply-send result and after-reply span. Previous processing starts when the original
writer receive returns and ends after its original match arm, including reply,
tail work and local drops. After-reply starts after the original send result and
ends at that same arm end. Its Ok means the original tail returned, while the
separate reply result describes response.send; neither proves subscriber delivery.
Previous work has no current-task identifiers, budgets or aggregate attribution.

Endpoint direction and checked microsecond distance are relative to the original
same-append send start. Sub-microsecond distances may round to zero without losing
their before/after direction. `receive_order` preserves receipt before, at or
after send return; there is no fabricated negative or zero wait. The writer's
`previous_work_relation` states completed by send start, overlapping send, or
starting at/after send return. Unobserved/Incomplete remain explicit. These direct
process intervals include thread scheduling. After-reply is nested in processing;
send overlaps queue. A single predecessor cannot establish queue depth, complete
waiting history, CPU/SQL cost or the cause of an uncovered gap. No query, extra
command, event, diagnostic channel or timing-history buffer supplies these fields.

Boundary samples have no record index. `last_call` adds the same call's budget
after return and its already known logical step/action. Identity is frozen before
the original clearing; a frame or recognition identity not yet issued is null.
The two append samples retain their common original frame/recognition identity.
All summaries use checked counters and microseconds in constant space. Parent
and child spans overlap and must not be added together.

`first_observed_expiry`, when present, retains the first completed interval
observed with a nonexpired Task budget at entry and an expired Task budget at
return. Completed inner append intervals are considered before their enclosing
append; later outer observations do not replace the first recorded interval.
This identifies a directly covered interval, not the first actual expiry instant
or a cause inferred across unmeasured gaps. Already-expired entry, entry-recovery
budgets, missing endpoints and incomplete aggregates cannot establish it. The
original four summaries retain their original endpoints and meaning.

An existing `runtime_connection` failure detail may contain fixed process-instant
`receive`, `validated_dispatch`, `policy_identity_projection` and `receipt_write`
observations, plus the current owner epoch and Runtime PID. These describe the
original read-frame, validated operation, projection return and write-frame
results; dispatch excludes receipt construction. They reset for each original
read, including idle reads, and are emitted only at that existing legal failure
point. A cached request has no observed dispatch/projection. Existing failure
deduplication or a Ledger failure can prevent that carrier; successful requests
and client timeouts alone create no new server fact. No server span proves that
the client received a receipt.

The existing Planning process failure output serializes only the original
header-I/O request/correlation/expected-owner/PID and native I/O kind/code. Its
already opened snapshot retains the original RuntimeFailed and lifecycle event
selection, with per-event request/correlation/known-owner comparisons and counts.
Null means unavailable, false means an observed mismatch. Zero matches within
those selected kinds cannot establish server progress, shutdown or full-event
coverage. No additional Runtime query, connection or request supplies evidence.

The optional `diagnostic_record_write.subphases` contains exactly `encode`,
`framing`, `capacity_admit`, `file_write` and `material_update`. Host measures the
original serde call (including its typed `to_value` conversion) and checked
framing/reserve/comma/newline work. ArtifactStream returns in-memory observations
of entered calls within that same single append: the complete original capacity
decision/binding/Drain check, the actual file write, and the original hash/count
update of only the successfully returned bytes. Each short-write iteration keeps
its own admission and file/material call in the fixed cumulative counts and last
sample. Host adds the actual record index and merges into the original phase/run.

Every subphase retains attempts/errors/total/max/last with checked microseconds
and accumulation. Unentered calls have `unobserved` status and absent elapsed
values; zero attempt/error/byte counts are counts, not measured zero durations.
File `successful_returned_bytes` sums only successful file returns, including a
zero return; its last `returned_bytes` is absent on Err. It does not claim how
many physical bytes an errored OS operation wrote. WriteZero and material errors
retain the original write failure even when the file call itself returned Ok.
Cleanup and other unmeasured overhead are outside these five items; their cause
cannot be inferred by subtracting subphase totals from the outer span.

The optional append observer is taken after the original append returns, before
owner abort, including on Err. Other stream calls keep observation disabled.
No state is borrowed from a previous append, stream or record. The same terminal
or permitted failure snapshot includes the last record's subphases; an absent
field in older facts means unrecorded and strict typed decoding remains in force.

Template deadline errors retain a typed `timing` observation through the original
recognition error and diagnostic error record: exact/coarse/refinement,
imageproc-returned or joint-template-color check stage, elapsed and limit in
microseconds. The original five-second deadline, check positions and messages
remain. Template PNG/ROI preparation precedes that deadline's original start.
Forensic event reads expose the optional typed fields; a run summary copies
`task_timing` only from its original terminal. Field absence means not recorded.

`request_id` identifies the actual contained-task execution. Scheduled runs also
retain the distinct `admission_request_id` from their validated PolicyRunContext,
with the original correlation/task/run identity. The task terminal keeps its
execution request link. An existing RuntimeFailed lifecycle carrier may retain
its admission request link only when that recorded admission ID and all three
correlation/task/run links match the observation. Neither request is rewritten,
and this association creates no additional event or bypass of failure handling.

`actingcommand.runtime.task-diagnostic.v1` is one immutable, task-scoped
`DiagnosticJson` artifact produced by the Runtime through ArtifactStore. Its
authority is the original GlobalLedger `ArtifactVerified` reference. Unpublished
staging bytes are not diagnostic evidence. The Runtime receipt keeps its existing
fields and artifact selection.

The Host opens one staging stream after task admission. Existing page, Home
preflight, guard and post-admission OCR evaluation points append their actual
results before the interpreter handles the original result or error. Evaluations
use the original Scene, provider calls, ordering, short circuits and unique-page
decision. The final record references existing verified screenshot, OCR,
stability and configuration artifacts by their source event sequence, native
identity and hash, using a fixed ledger cut. It does not capture or evaluate again.

Success and normally terminable failures/cancellations seal before the existing
task completion path. ArtifactStore reads back the entire stream with bounded IO,
verifies actual length/hash, and performs its existing atomic publication and
required created/verified event pair. Each task adds one artifact and O(1) new
artifact facts. A recording, seal, publication or cleanup error follows the
existing fatal/durability boundary. Fatal execution abandons the unpublished
stream; only earlier published facts remain provable. A killed process can leave
staging material without a published reference. No recovery log or background
publication is introduced.

## JSON framing and record bounds

The document is ordinary UTF-8 JSON with deterministic streaming framing:

- The first line contains the `TaskDiagnosticHeader` object fields, then
  `,"records":[` and LF, in place of the header's closing brace.
- Each `TaskDiagnosticRecord` occupies one compact JSON line. Every line after
  the first record starts with a comma. The footer is `]}\n`.
- The header binds request, correlation, task, run, instance and lease IDs to the
  artifact's publishing event. Indices start at 1 and increase without gaps.
  `parent_index`, when present, points to an earlier owning page, target or model
  result. Frame, source step action and physical action IDs are actual nullable
  links. Absence is retained; temporal proximity does not create an ID.

A serialized record is at most 1 MiB, excluding its comma and LF. OCR aggregate
and derived text each retain the existing 64 KiB limit; worst-case JSON escaping
uses at most 768 KiB for that pair. The remaining space holds the current ROI,
execution binding, IDs and metadata. Existing 4096-byte block/label strings and
1024-element provider result bounds remain unchanged. Blocks and labels are
separate records, so a legal multi-megabyte NN result does not become one large
record. Metadata that exceeds the record envelope fails explicitly; bytes are
never sampled or truncated. Serialization holds only the current source item,
its bounded encoded record and bounded provider-order indices. Each encoded
record is appended as a byte chunk; no whole-task document is assembled.

`kind` determines `data`:

The contract's `TaskDiagnosticPayload` is the discriminated transport type for
these twelve kinds. Host explicitly maps its existing evaluation values to the
contract DTOs; the contract has no dependency on evaluators or Host. Forensics
decodes the same `TaskDiagnosticRecord` and rejects unknown envelope/payload
fields and mismatched payload shapes. The private wire decoder resolves
`kind/data` before exposing a typed record. Nullable values stay nullable and
floating data uses `PartialEq`. Serialization retains the existing JSON number
precision within the current bounded record. Business results remain owned by
their original evaluators.

| Kind | Actual data |
| --- | --- |
| `page` | phase (`page` or `home_preflight`), native page index/ID, matched flag, group pass/total counts and message; errors carry the failed target and original typed cause |
| `target` | target ID/kind, passed/message, actual template and color evaluation, source role/group/target index or guard phase |
| `ocr` | requested/resolved region evidence, provider `raw_text`, business `derived_text`, aggregate confidence, actual selection/execution metadata and block count |
| `ocr_block` | original text/confidence/rect with `source_index` and `derived_rank` |
| `nn` | actual requested ROI, selected label/score, selection mode and label count |
| `nn_label` | original label/score with `source_index`; separate derived candidate flag/rank |
| `error`, `unexecuted` | existing cause or unexecuted reason at the original evaluation boundary |
| `step_started` | actual step index and RuntimeClock monotonic timestamp |
| `step_elapsed` | actual start/end timestamps, checked monotonic difference and completion flag |
| `artifact` | original ArtifactVerified sequence and projected native artifact reference |
| `terminal` | interpreter result or original error code; unavailable executed-step counts are null |

`executed_steps` is the run owner's count of dispatched logical steps. A dispatched
step counts even if its guard, input or postcondition fails; retries of that step
retain its index. It is distinct from physical input count and successful
postcondition count. Phase transitions retain the global run count. Entry recovery
contributes its own run count exactly once before the target run's count.

Kernel snapshots this count at run start and logical-step dispatch through the
existing Runtime boundary. Host uses that same snapshot for error/cancellation
task terminals and diagnostic terminals, including errors while recording the
result. A known pre-dispatch state is zero. If progress cannot be obtained, such
as an interrupted run recovered after restart, `task.terminal_committed` retains
`executed_steps: null`; a successful terminal requires a known count. No progress
is inferred by counting ledger completion events, and original errors and
committed effects remain unchanged.

Task-error terminals retain the optional typed `timing` observation from the
Kernel's original decision. `scope` identifies the task budget, page-recognition
wait or postcondition wait; `stage` identifies the decision point. `elapsed_ms`
and `limit_ms` use the owner's monotonic clock and milliseconds. A present
`required_delay_ms` means the remaining task budget could not admit that wait;
otherwise the measured limit had elapsed. An error page or another cause has no
timing observation. The original error code/detail and retry/recovery decision
remain authoritative. Older or unavailable timing remains absent, without a
zero substitute. These observations do not identify a device or provider cause.

An unfinished step's terminal retains its existing step action ID, linking the
original StepStarted operation, phase and global index to its actual elapsed
record. Pre-dispatch failures have no step action. Completed steps and stability
terminals retain their existing step events and verified comparison references.
Stability comparison artifacts include the declared `max_steps` alongside the
threshold, counter transition, reason and exact frame pair. Read-side projection
leaves `max_steps` null when an existing artifact has no such fact.

Model blocks and labels are written in provider order. Business sorting and
selection retain their original behavior. Template raw/normalized score,
threshold and hit rectangle and color mean/expected/distance/max-distance come
from the existing evaluator. No overall page score or NN bounding box is invented.

Step intervals begin at the actual StepStarted callback and end at StepFinished,
covering guard, input and post-input waiting. An unfinished attempt ends only at
the next actual attempt or task termination, with `completed=false`; this does
not emit StepFinished. RuntimeClock `checked_sub` supplies elapsed time. Wall
clock values do not supply or correct the interval.

## Read-only export and privacy

`actingledger export --task-evidence` retains the existing ledger event window.
`--record-limit N` bounds the total expanded records in that page (default 16,
range 1–64). Each diagnostic page provides its own `next_cursor`; pass its JSON
unchanged as `--record-cursor JSON` together with the same ledger window. A cursor
binds the exact artifact ID and SHA-256 plus the last emitted record index.
It selects that artifact only. The other artifact pages retain their own cursors,
including index zero when the current record budget did not reach them.

The reader scans records with a bounded line buffer and retains only the selected
page. It validates framing, identities, record order, length and full artifact
hash before returning `state=verified`. Each page rereads the immutable artifact
to verify its complete contents; it never claims a verified prefix from an
unchecked suffix. Event paging and record paging remain separate continuations.
`window_complete` retains the event/input-chain window meaning. Diagnostic
coverage is reported separately by each artifact's state, total and cursor and
by `diagnostic_gaps`; it must not be inferred from that window flag.

Raw task diagnostics use the existing `Pending` redaction policy. Ordinary
export returns `privacy_withheld`, without header, records or a claimed count.
`--include-private` explicitly enables local controlled reads; it does not mark
the artifact public or change personal/source projection rules. Original OCR,
stability, online-observation, Lab operation and effective-configuration schemas
can be expanded from their existing verified references as one legacy record,
up to the same bounded 1 MiB document envelope. Larger legacy documents remain
native references with an explicit read-limit failure. Unknown schemas remain
references with `unknown_schema`, never a guessed interpretation.

Missing, unsealed, malformed, wrong-identity or damaged artifacts are explicit
failures/gaps. A run without an accessible task diagnostic reports
`not_recorded_or_unpublished_or_withheld_or_outside_window` and an unknown record
count. Missing facts cannot establish how many evaluations preceded a fatal
termination. Reading uses GlobalLedger's public read-only surface and does not
take writer locks, repair files or publish staging material.
