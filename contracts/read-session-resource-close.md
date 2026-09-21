# Observation session resource close

Readonly observations can retain execution sessions without a business input
lease. Host closes those resources through the same Scheduler that owns business
leases. After shutdown stops admission and drains its workers, Host enumerates
Kernel-owned instances and acquires a current, exclusive resource-close lease for
each remaining session. Grant and release use the existing critical ledger and
OwnerGuard transitions.

The Scheduler marks this lease as resource-close-only. Epoch, connection, lease
identity, expiry, cooldown and destructive-step checks remain effective.
`begin_resource_close` admits the native close and returns its `FencedWrite`;
ordinary input validation rejects this lease. Host invokes
`close_instance(FencedDeviceWrite(Arc<FencedWrite>))` only after that
admission, persists resource quiescence, and releases the lease after confirmed
retirement. The operation produces no game input.

The witness contains the complete admitted token identity, connection identity,
nonzero scheduler-local step ID and ResourceClose purpose. It has no Copy, Clone,
Default or serialization implementation. The contract's single issuing bridge is
public across crates: the source guard restricts its production callers to the
Scheduler's two begin methods. Private fields prevent ordinary field construction;
they do not provide a Rust friend-crate guarantee. Device and Kernel have no
Scheduler dependency.

Host keeps the consumable root. Kernel and Nemu commands carry short-lived Arc
references to that same step, and synchronous close operations borrow its witness.
Every normal or failed response releases its command/context/check references
before sending. This includes Business input/application failures before entering
the retained-close loop. Host recovers the unique root on an originally permitted
finish branch and passes the value to Scheduler, which checks the current step
identity and consumes it. Finish does not add an expiry check. Missing references,
lost responses and unconfirmed effects never permit a replacement witness, forced
step clearing, delayed retry or automatic finish on Drop.

ResourceClose permits the original ordered close of both backends even when the
first fails. A Business witness cannot authorize a device-effect close. LocalOnly
retains its original local-cleanup semantics. Cached close results contain outcome
and occurrence data, never a witness. Business write signatures beyond the shared
begin/finish and worker lifetime remain governed by their existing checks.

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

If one instance remains unconfirmed, the same retained OwnerGuard handle still
records the retirement of other instances. Its overall Unconfirmed disposition
stays fixed and its OS lock remains retained after Host shutdown. Journal failures
continue to fail explicitly; a successful sibling close cannot clear that state.

Resource quiescence has the existing Runtime-owned resource boundary described in
`nemu-owned-resource-close.md`. SDK global state and a real-device recovery remain
outside CI's proof.
