# Input receipt wait

The RuntimeClient receipt selector is the budget owner for direct and debug
input, SafeReset, and ReleaseLease. Callers submit one request and await its
original terminal receipt; a timeout or disconnected transport remains latched
and does not authorize replay.

Let `B` be the existing configured `backend_open_timeout`, `I` the configured
`io_timeout`, and `G` the explicit gesture duration (the sum of all phases for a
segmented gesture). The client uses these finite response wait allowances:

| Operation | Response wait |
| --- | --- |
| Input, including tap, key, text and reset | `B + G + B + I` |
| SafeReset, which includes input and resource release | `B + B + I` |
| ReleaseLease | `B + I` |

The first backend allowance covers opening the input backend. The second allows
the input owner to settle a failure and close retained resources before returning
the original error. ReleaseLease needs the close allowance and transport margin.
All duration sums are checked; overflow is a visible error before submission.
With the existing defaults (`B = 60s`, `I = 5s`), tap waits at most 125s and
ReleaseLease waits at most 65s for a response read. Explicit gesture duration is
added without changing its requested execution time.

These are client waiting allowances. Host leases, fencing, backend command
deadlines, geometry validation, recovery attempts and resource-close authority
remain unchanged. Other operation budgets retain their existing selection.
The shared operation/close error presentation names an input backend and retains
the primary diagnostic, sensitivity, ADB recovery record and resource causes.

The existing typed input, debug correlation, reset failure, receipt-selector,
latched transport and close-combination specifications include the Workflow
#284 B14 regression. Scaled I/O and backend allowances preserve the 5:12 ratio;
an open or close can outlive I/O while its original terminal remains receivable.
The exhaustion case preserves uncertainty about an already submitted input and
checks that subsequent client calls do not submit it again. Validation is CI-only.

The existing mapped-successor specification correlates one captured event set.
It preserves exactly one source input and the unique outcome-driven successor.
Every observed input belongs to one of those runs and has its prior policy
admission, matching lease grant, Scheduler request admission and original action
intent. The successor's own single-step input is distinct from an independent
wake effect; the specification does not rely on a changing global input count.

## Single-input command receipts

RuntimeClient and its Debug session return the original validated RuntimeReceipt
for a committed input. RuntimeInputAuthority and RuntimeInputProxy carry that
value through the same call. The CLI input adapter exposes it for the current
single tap/swipe/long-tap/key/text command; shared unit-returning input interfaces
and multi-action consumers retain their existing semantics.

The command owns the returned value before it closes the proxy. Its success data
or LabError details contain input_outcome, also displayed in human output:
input_stage is committed when the current call returned its input receipt,
receipt_unavailable when that call failed without one, or not_submitted when
command preparation failed before input was invoked. input_receipt retains the
original request_id, correlation_id, state, terminal sequence/event_id and
InputCommitted.action_id. close_stage records succeeded or failed after the
existing close call, with the original close_error on failure. Preparation
failure makes no claim about a subsequent close phase.

The existing operation/close combiner preserves all four outcomes: both successes
return the command result; either failure remains an error; both failures retain
the input error followed by the close error. A close failure still exits with
code4. Receipt serialization failure also fails visibly, retaining any original
operation/close error and its serialization error in details. No last-input cache,
reconnection, resubmission, ledger lookup or extra input is introduced.

Committed is the receipt stage. Physical effect disposition is established by the
original InputPayload referenced by its terminal; this projection does not infer
performed or NotPerformed from InputCommitted. A receipt for one input does not
assert completion of a stream, recovery sequence or other multi-action command.

The existing typed-client, Debug correlation, heartbeat proxy, transport latching
and production-tap specifications cover the return path and original effect
counts. The existing four-quadrant combiner specification retains its error and
resource-close assertions. The production-tap fixture always closes successfully;
it does not execute the historical closing-failure branch. The original Workflow
#284 B32 close failure and #302 Unconfirmed boundary remain preserved. All current
validation runs through CI; no new fixture or failure-injection interface is added.

## Receipt-header I/O context

A failed four-byte receipt-header read retains its original I/O ErrorKind,
optional raw OS code and up to 256 Unicode characters of the original message in
RuntimeClientError.receipt_header_io. message_truncated explicitly marks a longer
message. The current exchange attaches its sent request_id/correlation_id and the
expected owner_epoch/PID frozen in this connection's RuntimeInfo. These owner
fields are connection metadata, not a new observation of another process or a
claim about its shutdown. A successful connection's existing health check verifies
that epoch; an initial health failure retains only the expected discovery identity.
Lower-level exchanges without those values keep None.

The existing error code, operation, fatal/related and committed_receipt meanings
remain. A deadline mapping may retain an original UnexpectedEof kind even when its
code is the existing receipt-timeout code. The fields are displayed by the existing
Debug/Display error paths, including planning assertion failures and CLI errors;
no request body, token, authentication frame, configuration or extra query is added.
The latched error keeps the original failed request context on later refused calls.
If timeout restoration also fails, the existing restoration error stays primary
and the actual header error remains related.

A client read failure does not establish Runtime Fatal, request non-processing or
physical non-performance. The normal 500ms planning reader and existing
ReceiptReadDeadline behavior are unchanged. Existing EOF and broken-IPC
specifications retain their deadlines/no-reconnect/no-resend assertions while
checking direct cause/known identities and unknown values. The original P6 failure
has no preserved original I/O kind or current-child request association; a successor
result cannot supply those facts retroactively. Untriggered restoration and message
truncation branches retain their actual CI coverage boundary.
