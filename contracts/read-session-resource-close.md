# Observation session resource close

Readonly observations can retain execution sessions without a business input
lease. Host closes those resources through the same Scheduler that owns business
leases. After shutdown stops admission and drains its workers, Host enumerates
Kernel-owned instances and acquires a current, exclusive resource-close lease for
each remaining session. Grant and release use the existing critical ledger and
OwnerGuard transitions.

The Scheduler marks this lease as resource-close-only. Epoch, connection, lease
identity, expiry, cooldown and destructive-step checks remain effective.
`begin_resource_close` admits the native close; ordinary input validation rejects
this lease. Host invokes `close_instance(FencedDeviceWrite)` only after that
admission, persists resource quiescence, and releases the lease after confirmed
retirement. The operation produces no game input.

Readonly, monitor and contained-task capture failures share this close path under
the instance admission mutex. They can reuse a current business lease only when
its existing close checks permit it. That lease remains with its original cleanup
owner. A dedicated close lease is released by the same Host cleanup implementation.
The primary capture failure and any actual cleanup cause retain their existing
typed GlobalLedger representation.

An earlier fatal does not skip other instances' permitted cleanup. An expired or
otherwise invalid lease, an unresolved destructive step, or an unconfirmed native
close retains failure and owner protection. Final Kernel shutdown aggregates cached
unconfirmed causes and retains any session whose close authority was not established;
it cannot retry that session through a local-only native close. The first native
close result remains stable.

Resource quiescence has the existing Runtime-owned resource boundary described in
`nemu-owned-resource-close.md`. SDK global state and a real-device recovery remain
outside CI's proof.
