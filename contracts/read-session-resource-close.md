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
and occurrence data, never a witness. Business input and application-lifecycle
writes take the witness as described in the section below; their existing checks
remain.

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

## Input and application-lifecycle writes take a FencedWrite

Every device input and application-lifecycle write entry point requires the
Business witness that `begin_destructive_step` issued for the step:

- Device: the `InputBackend` write methods (`tap`, `tap_in_frame`, `long_tap`,
  `swipe`, `segmented_swipe`, `segmented_swipe_prepared`,
  `segmented_swipe_prepared_in_frame`, `key`, `text`, `reset`) take
  `&FencedWrite` as their leading parameter, as do the ADB write methods
  `shell_input_tap`, `shell_input_swipe`, `force_stop` and `launch_package` and
  the device helpers that drive writes (`replay_input_records`,
  `validate_maatouch`). `Adb::run` serves read-only commands and the
  connection-time commands of backend opening; a command with a device effect
  goes through `Adb::run_write(&FencedWrite, args)`. A backend treats the witness
  only as the type-level proof that a step was begun; validating its content
  belongs to the Scheduler. Wrappers pass the caller's witness through unchanged.
- Kernel: the session's input and application-lifecycle commands carry a
  required `Arc<FencedWrite>`, and the public `ExecutionKernel` and
  `ExecutionSession` input and `control_application` entry points take it. The
  Kernel neither issues nor copies a witness: the `Arc` only carries the Host's
  step across the session thread, which lends `&FencedWrite` to the device write
  and to `ExecutionBackendProvider::control_application`.
- Host: the two `begin_destructive_step` sites (input and application lifecycle)
  pass the witness they issued. Contained-task input and application effects
  reach the device only through those two sites. A failed
  `begin_destructive_step` reaches no write path.

Boundary of this step: connection-time writes of backend opening (`connect`,
`push`, `chmod`, `forward`, `shell_spawn`) stay under the existing open-phase
authority; read-only operations (capture, `dumpsys`, `wm size`, version probes,
foreground observation) take no witness; the close side is unchanged and keeps
`DeviceCloseAuthority`. Host position-only checks remain only on non-write paths
(below). Witness semantics, the Scheduler's issue and consume, wire formats and
the ledger's `input.committed` sequence are unchanged.

Source guards pin the table: the production callers of `issue_fenced_write` are
exactly `SeedScheduler::begin_destructive_step`, which mints Business, and
`SeedScheduler::begin_resource_close`, which mints ResourceClose. In `crates/device`
and `crates/execution-kernel`, every `InputBackend` method other than its
read-only accessors and closes, every public `Adb` method other than its
read-only and open-phase commands, and every Kernel `input*` /
`control_application*` entry takes a `FencedWrite` parameter; `close_once`,
`close_with_authority`, `close_with_input_check` and every public function
calling one take a `DeviceCloseAuthority` parameter unless they pass only
`DeviceCloseAuthority::LocalOnly`.

### Lab port and the lease field

Lab reaches input only through `SemanticInputExecutor`. Inside `crates/lab`, only
its implementations call the `LabInputPort` write methods; an env-detection touch
step goes through the factory-backed implementation, which opens one port for
the action and closes it before returning. `crates/lab` names no device input
backend (`InputBackend`, `Adb` or a touch backend). A source guard pins both.

`InputBackendRequest.lease` carries a lease the caller already holds. With
`Some`, the ActingLab Runtime port submits each input under that lease and
acquires, renews and releases none: the holder does, and the port reports
`lease_held`. With `None`, the port acquires its own lease through
`RuntimeInputProxy` as before. A lease belongs to the Runtime connection that
holds it; the Runtime refuses one presented on another connection with its
existing lease denial. In both cases Lab holds no witness: the Runtime validates
the lease and mints the `FencedWrite` for each input. Offline and fixture paths
are unchanged.

### Position validation remains only on non-write paths

Host lease validation by position (`validate_write` without a witness) remains
only where it admits a request before any witness exists:
`HostShared::validated_instance`, used by lease renewal and release, input and
application admission, contained-task admission and the Lab-operation capture
fences. It authorizes no device write. The Nemu input check validates only the
step's witness (`validate_destructive_step`); its former second `validate_write`
on the same token repeated that validation and is removed. The Scheduler's own
`validate_write` inside `begin_destructive_step` remains the Business mint's
admission. The ledger event sequence is unchanged.
