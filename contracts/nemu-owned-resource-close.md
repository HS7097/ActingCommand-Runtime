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
