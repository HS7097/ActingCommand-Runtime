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
