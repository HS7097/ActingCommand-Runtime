# C3a Resident Runtime Seed Implementation Plan

## Authority and scope

This plan implements Issue #35 C3a from:

- `TASK-runtime-ledger-core-and-optional-lab-correction-v3.md`
- `docs/architecture/runtime-ledger-v3-c0-freeze.md`

C3a is the first production slice of the terminal scheduler and Runtime host. It is not a
temporary mock. The implementation must grow in place during C3b.

In scope:

- one resident Runtime owner with OS-held exclusion and crash takeover;
- a new `owner_epoch` for every successful start;
- length-delimited typed local IPC;
- per-instance write admission and leases;
- grant, renew, release, expiry, busy rejection, and idempotent renew/release;
- daemon-owned input backends behind DeviceProxy;
- fencing on every input operation;
- connection-bound backend guards and lease cleanup;
- takeover cooldown greater than the maximum client heartbeat interval;
- global-ledger events and correlated receipts;
- a read-only capture capability that cannot recover input authority.

Out of scope:

- queueing, priority, preemption, scheduling policy, task lifecycle, or task execution;
- daemon-owned capture or recognition;
- remote API, UI, game logic, OCR, SQLite, artifact bytes, or resource changes;
- C3b execution-kernel ownership.

## Frozen defaults

- maximum client heartbeat interval: `5_000 ms`;
- takeover cooldown: `6_000 ms`;
- write lease TTL: `120_000 ms`;
- IPC request/response maximum: `1 MiB`;
- IPC read/write timeout: `5_000 ms`.

Configuration that does not satisfy
`takeover_cooldown_ms > maximum_client_heartbeat_interval_ms` is fatal.

## Task 1: Runtime, scheduler, lease, and DeviceProxy contract

Status: complete.

- Add typed `OwnerEpoch` and `HolderId` identifiers.
- Add a validated Runtime request envelope carrying request/correlation identity,
  actor/source, submitted wall-clock time, and a tagged operation.
- Add typed acquire/renew/release/read-only/input operations, lease tokens, receipts,
  machine-readable errors, Runtime info, and input actions.
- Add a verified-request boundary that can construct event links without exposing generic
  transport-ID promotion.
- Extend the event vocabulary for Runtime start/takeover and lease renewal.
- Add serialization, validation, secret-channel, and capability negative tests.

## Task 2: Per-instance scheduler seed

Status: next.

- Create `crates/scheduler` without queue/preempt/task behavior.
- Partition state by typed `InstanceId`.
- Implement one write lease per instance, fixed TTL, monotonic expiry, busy rejection,
  idempotent renew/release, connection ownership, and takeover cooldown.
- Validate `owner_epoch`, `lease_id`, `instance_id`, `holder_id`, connection guard, expiry,
  and cooldown before every write.
- Add zero-stagger same-instance and independent-instance concurrency tests.

## Task 3: Runtime host, owner guard, IPC, and DeviceProxy

- Create `crates/runtime-host`.
- Acquire one OS-held owner file and append durable owner metadata. Persist active instance
  IDs so crash takeover can restore cooldown only for affected instances.
- Open one GlobalLedger writer, bind loopback TCP, publish `runtime-info.json`, and accept
  length-delimited typed requests.
- Keep scheduler, owner metadata, backend registry, and fatal health state in the host.
- Open input backends only inside the daemon. Lock per-backend, revalidate fencing
  immediately before every input call, and use C1 critical ordering for input and lease
  transitions.
- Explicit connection close, protocol failure, disconnect, and host shutdown must close
  backend guards, revoke leases, update owner metadata, and append events. Cleanup is
  idempotent; cleanup failure makes host health fatal.

## Task 4: Runtime client and actingd process

- Create `crates/runtime-client` and `apps/actingd`.
- Runtime client discovers the daemon from `runtime-info.json`, keeps one local IPC
  connection, and exposes health, read-only admission, lease, input, and event-query calls.
- Add a heartbeat-backed `RuntimeInputProxy` implementing the existing `InputBackend`
  interface without constructing a local device backend.
- `actingd` remains a thin process adapter. It loads a typed local config, builds the input
  registry and Runtime host, maps fatal startup/run errors to a nonzero exit, and contains no
  scheduler or device policy.

## Task 5: Remove client-side production input construction

- Replace ActingLab's production `AppInputFactory` and direct write helpers with
  `RuntimeInputProxy`.
- Preserve dry-run and sealed fake-input paths.
- Keep capture client-side and read-only in C3a.
- Add architecture guards rejecting direct touch-backend construction in production client
  sources and rejecting any writable method/downcast/getter on the read-only capability.

## Task 6: C3a adversarial and process acceptance

- Prove zero-stagger same-instance requests produce one grant and one busy denial.
- Prove different instances acquire and execute independently.
- Prove every input action passes through DeviceProxy and is rejected before backend use for
  stale epoch, wrong lease, wrong instance, wrong holder, wrong connection, expiry, and
  cooldown.
- Hard-kill a daemon with an active lease, restart it, verify a new epoch, reject every old
  token input variant, enforce takeover cooldown, then grant after cooldown.
- Drop a client without release and prove its backend closes and lease is revoked.
- Query the global ledger and prove request -> admit -> grant -> input intent/outcome ->
  release is one correlated sequence.
- Prove the read-only capture capability has no input interface or writable recovery path.

## Task 7: Closeout

- Run focused contract/scheduler/host/client/process tests.
- Run full workspace tests, non-Lab workspace tests, all-features dependency guards, Clippy,
  formatting, forbidden-source scans, and `git diff --check`.
- Perform a fresh whole-C3a review and fix every Critical or Important finding.
- Update `PLANS.md` and `CHECKPOINT.md`, commit and push each completed implementation unit,
  create a checkpoint tag, and record evidence in Issue #36.
- Do not merge into `main` or the umbrella repository.
