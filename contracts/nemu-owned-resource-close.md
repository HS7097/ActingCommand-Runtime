# Nemu owned-resource close

Nemu close accounts for the Runtime's exclusive opaque connection, serial capture
worker, vendor stdio session and loaded library reference. It runs under the
existing fenced device-write close authority. The emulator and SDK global threads
are outside this owned-resource boundary.

The worker completes each earlier capture before handling shutdown. It resolves
`nemu_disconnect`, calls it once for the owned connection and records its actual
return by retiring that connection ID. Subsequent close reads use the cached close
result and cannot call the retired connection again. A void return supplies no
independent provider termination acknowledgement; no vendor guarantee about global
SDK state is inferred from this ABI.

The owner then finishes vendor stdio, closes its library reference and joins its
worker within the existing bounds. Only the complete successful local chain yields
owned-resource quiescence. Host persists the stdio close observations through its
existing lifecycle path before the typed resource-quiescence fact and lease
release. Lifecycle failure and M4 summary facts retain their existing paths.

Symbol resolution, call wrapping, stdio restore/descriptor close, unload, channel and join failures preserve
their real causes. A timed-out or still-running worker remains unconfirmed, and
Host retains its existing Unconfirmed/Fatal and OwnerGuard behavior. A returned
disconnect call does not complete those remaining resources. The first close
result, including a failure, is stable on repeated reads.

CI specifications establish the owned Rust/OS close chain with an in-test void
callback. Actual SDK closure, subsequent lease release and Lab arrival require the
assigned real-device window. SDK postconditions beyond the observed call return
remain unverified.

Workflow #191 H: the Nemu session stays open across leases, so its stdio redirection
and the process-level stdio lock are held per native call, not for the session's
life. The session starts redirected and holding the lock (the library load is its
first call); each later native call (`nemu_connect`, `nemu_get_display_id`,
`nemu_capture_display`, the finger down / up events, `nemu_disconnect`) resumes the
redirection (takes the lock, installs the capture targets again), makes the call,
reads its snapshot and suspends (flushes, restores FD1/2 and the Win32 standard
handles to the saved originals and releases the lock); the saved and capture
descriptors, the capture files and the library stay with the session. Vendor output
during a call is captured as before; output outside a call, and the daemon's own
output, reaches the daemon's real standard streams. A resume that cannot redirect
undoes its partial install at once and skips the call, except for the release calls
(`nemu_disconnect`, finger up), which are made anyway; either failure is reported.
A failed restore is unconfirmed, keeps the lock and is the session's cached failure,
reported again by its close. Library unload stays after the stdio close, outside any
redirection, as before.

The stdio owner retains a bounded private context for its existing acquisition,
redirection, restoration, descriptor close and path unlink operations. The fixed
set consists of FD1/2, two saved descriptors, two capture descriptors and the two
Win32 standard-handle table slots. A complete successful path has 32 recorded
operations: the acquisition, the first installation and its restoration, and the
final descriptor closes and unlinks. Later resume/suspend cycles are not counted in
this 32-step budget: each records into its own context, which is dropped when the
cycle completes; a cycle that fails keeps its own context, which its error and the
session's close report. The context stores at most 32 steps (up to three reference observations
per step); overflow increments a saturating dropped count. Frame snapshots do not
append steps. PID, process creation FILETIME and observation FILETIMEs distinguish
the producing process and read timing. Each step carries its API, phase, roles,
actual return and immediately captured CRT/Win32 error; unlink uses the existing
Rust I/O result (0 for Ok, -1 for Err) and its original optional OS code.

Capture files use non-inheritable handles with read/write access and
read/write/delete sharing. Each installed standard descriptor has its native
inheritance bit cleared and its actual resulting flags recorded. Restore compares
the current target handle and file identity with this session's installed target,
then explicitly closes the owned CRT target before `_dup2` restores it. The
original observed inheritance bit is restored on that new reference. Unknown
ownership, retirement failure or restore failure retains Unconfirmed. Saved
descriptors are preserved until their existing close step. There are six recorded
FD closes; owned-resource counts retain their existing meaning.

Handle flags come from GetHandleInformation; file identity is the volume serial
and 128-bit file ID returned by GetFileInformationByHandleEx(FileIdInfo). These
queries use only currently held descriptors. Metadata unavailable on a console,
pipe or invalid handle remains typed unknown with the original query error.
An unsuccessful handle-information query stops metadata reads for that handle;
file identity records HandleUnavailable with that query error.
Borrowed Win32 table values have unknown metadata unless the same observation
associates them with a currently held owner FD; metadata then names that FD as
its source. Matching handle numbers alone is not file identity. File identity
identifies a file, not every reference or external process that can keep it open.
After any descriptor close attempt its retired reference is never queried or
closed again, including when close failed. A successful `_dup2` creates a new
owned reference in the target slot whose metadata may then be observed.
Subsequent table observations read only the borrowed raw value unless associated
with a currently owned reference.

Each unlink records its exact UTF-16 path and Removed or Residual result. Residual
preserves the original I/O error and does not itself make resource quiescence
Unconfirmed. A completed stdio close can proceed to the existing library close;
it releases the process-level stdio lock if a call still held it. Restore,
descriptor, disconnect and library failures retain their real causes and existing
failure protection.

The vendor text the Nemu worker captured is bounded: between two frames it keeps at
most 64 KiB in at most 256 snapshots, dropping the oldest first (a single oversized
snapshot keeps its tail), and writes each drop into the text itself as a leading
`…[N bytes dropped]` line per stream. A frame carries only the text captured since
the previous frame; the worker keeps no copy.

An unlink failure triggers one Restart Manager session for the current capture
paths: StartSession, RegisterResources, at most two GetList calls, and EndSession.
The list is capped at 16 entries. API statuses, required/reported counts and reboot
reasons are retained; oversize, races, API/access errors and session-end failures
are explicit Incomplete/Unavailable observations. Only a completed successful
empty observation describes no reported holders. Each returned process carries
its PID, creation FILETIME (UTC 100 ns ticks since 1601-01-01) and the original
Restart Manager application/service display name. The name is not executable
identity. The probe performs no shutdown, restart or holder termination. Normal
successful unlink does not run it.

The context travels on the existing DeviceResourceCloseOutcome through Kernel
to the Host lifecycle ledger event. Residual observations use Warning severity.
Later library/worker/backend errors retain already obtained observations through
the existing error context, while actual stdio failures retain their original
DeviceResourceCloseCause. Occurrence identity and cached results prevent repeated
close from probing or emitting the same observation again. Facts are sensitive
and absent from public projections. Ledger append or synchronization failure is
a program failure and stops further writes. Native observation failure cannot
replace a close error or confirm release. These observations describe the current
session; past external-reference ownership remains Pending Verification.
