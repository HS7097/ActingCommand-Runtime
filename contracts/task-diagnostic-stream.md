# Task diagnostic stream

The existing task terminal may carry `task_timing`, a bounded observation of
`recognition_evaluate` and `diagnostic_record_write`. Each has separate preflight,
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
