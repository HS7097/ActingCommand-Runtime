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
owned-resource quiescence. Host persists its existing typed resource-quiescence
fact before lease release; lifecycle failure and M4 summary facts use their
existing paths and schemas.

Symbol resolution, call wrapping, stdio, unload, channel and join failures preserve
their real causes. A timed-out or still-running worker remains unconfirmed, and
Host retains its existing Unconfirmed/Fatal and OwnerGuard behavior. A returned
disconnect call does not complete those remaining resources. The first close
result, including a failure, is stable on repeated reads.

CI specifications establish the owned Rust/OS close chain with an in-test void
callback. Actual SDK closure, subsequent lease release and Lab arrival require the
assigned real-device window. SDK postconditions beyond the observed call return
remain unverified.

The stdio owner retains a bounded private context for its existing acquisition,
redirection, restoration, descriptor close and path unlink operations. The fixed
set consists of FD1/2, two saved descriptors, two capture descriptors and the two
Win32 standard-handle table slots. A complete successful path has 24 recorded
operations. The context stores at most 32 steps (up to three reference observations
per step); overflow increments a saturating dropped count. Frame snapshots do not
append steps. PID, process creation FILETIME and observation FILETIMEs distinguish
the producing process and read timing. Each step carries its API, phase, roles,
actual return and immediately captured CRT/Win32 error; unlink uses the existing
Rust I/O result (0 for Ok, -1 for Err) and its original optional OS code.

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
After any descriptor close attempt that FD is never queried again, including when
the close failed; subsequent table observations read only the borrowed raw value.

On failure the context travels on the existing DeviceResourceCloseCause through
Kernel to the Host lifecycle ledger event. It is sensitive and absent from public
projections. The original occurrence, primary error, severity, quiescence, close
cache and lease release decision are unchanged. Observation failure cannot replace
a close error or confirm release. Acquisition facts describe the references leading
to a later close cause; they do not create a separate startup or sampling event.
The context neither enumerates other owners nor reconstructs a past process's
blocking references. External reference ownership remains Pending Verification.
