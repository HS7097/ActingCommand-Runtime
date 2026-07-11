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

Status: complete.

- Create `crates/scheduler` without queue/preempt/task behavior.
- Partition state by typed `InstanceId`.
- Implement one write lease per instance, fixed TTL, monotonic expiry, busy rejection,
  idempotent renew/release, connection ownership, and takeover cooldown.
- Validate `owner_epoch`, `lease_id`, `instance_id`, `holder_id`, connection guard, expiry,
  and cooldown before every write.
- Add zero-stagger same-instance and independent-instance concurrency tests.

## Task 3: Runtime host, owner guard, IPC, and DeviceProxy

Status: complete.

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

Implemented result:

- `actingcommand-runtime-host` owns one OS-locked owner journal, fresh owner epochs,
  crash-takeover instance cooldowns, one GlobalLedger writer, loopback TCP framing, and
  `runtime-info.json` discovery.
- Input backends are opened and retained only by backend worker threads inside the host.
  Every input variant is re-fenced immediately before use and follows durable intent,
  action, durable outcome ordering.
- Connection drop, explicit release, expiry, shutdown, and backend failure close the
  backend guard and revoke the matching lease. Cleanup and request replay are idempotent.
- Repeated acquire after the bounded connection cache is unavailable recovers its original
  durable `LeaseGranted` terminal instead of appending duplicate request facts.
- Nine host tests and fourteen scheduler tests cover lifecycle, fencing, idempotency,
  takeover metadata recovery, malformed owner metadata, expiry, disconnect, redaction,
  and correlated ledger events.

## Task 4: Runtime client and actingd process

Status: complete.

- Create `crates/runtime-client` and `apps/actingd`.
- Runtime client discovers the daemon from `runtime-info.json`, keeps one local IPC
  connection, and exposes health, read-only admission, lease, input, and event-query calls.
- Add a heartbeat-backed `RuntimeInputProxy` implementing the existing `InputBackend`
  interface without constructing a local device backend.
- `actingd` remains a thin process adapter. It loads a typed local config, builds the input
  registry and Runtime host, maps fatal startup/run errors to a nonzero exit, and contains no
  scheduler or device policy.

Implemented result:

- `actingcommand-runtime-client` discovers and validates `runtime-info.json`, owns one
  loopback IPC connection, correlates typed receipts, latches terminal transport/protocol
  failures without reconnect, and exposes health, read-only admission, lease, input, and event
  query methods.
- `RuntimeInputProxy` implements `InputBackend`, renews its lease on a bounded background
  heartbeat, serializes heartbeat and input over the same connection guard, extends only known
  long-action response waits, and releases the lease on explicit close or drop.
- `actingd` accepts one typed JSON config, requires an explicit touch backend so no unreported
  automatic fallback occurs, assembles the existing host/provider modules, reports fatal errors
  with exit code 1, and owns no scheduling or device policy.
- Process acceptance starts the real daemon, closes one disposable client, attaches a second
  client to the still-running daemon, and verifies invalid startup exits nonzero.

## Task 5: Remove client-side production input construction

Status: complete.

- Replace ActingLab's production `AppInputFactory` and direct write helpers with
  `RuntimeInputProxy`.
- Preserve dry-run and sealed fake-input paths.
- Keep capture client-side and read-only in C3a.
- Add architecture guards rejecting direct touch-backend construction in production client
  sources and rejecting any writable method/downcast/getter on the read-only capability.

Implemented result:

- ActingLab production input paths now acquire `RuntimeInputProxy` from the resident Runtime
  and no longer construct MaaTouch, minitouch, or ADB-shell input backends in client code.
- Direct tap, long-tap, swipe, key, text, stream relay, semantic input, and the Lab input
  factory use the same lease-gated Runtime boundary. Dry-run and sealed fake-input paths remain
  local and unchanged.
- Capture remains a client-side read-only capability for C3a. Its public capability surface is
  guarded against writable methods, downcasts, public fields, and unrelated trait exposure.
- A real-process ActingLab test proves a tap reaches a fake daemon-owned backend without any
  client ADB configuration. Source and dependency guards reject production backend constructors
  in ActingLab and runtime-client sources.

## Task 6: C3a adversarial and process acceptance

Status: complete.

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

Implemented result:

- Host-level zero-stagger clients produce exactly one grant and one machine-readable busy
  denial, while different instances both acquire, execute, and release independently.
- Scheduler and DeviceProxy tests cover every fencing field before backend use. The ordinary
  host path executes every current input action through the daemon-owned backend.
- A sealed child process is hard-killed with an active lease, restarted against the same owner
  journal, and verified to issue a new epoch. Every old-token input variant is rejected as stale
  without opening a backend; takeover cooldown denies acquisition until its reported delay has
  elapsed, after which a new lease executes normally.
- Dropping a raw IPC client and dropping `RuntimeInputProxy` both close the backend and revoke
  authority without stopping the resident Runtime.
- Acquire, input, and release requests sharing one typed correlation ID project as one ordered
  ten-event sequence from request/admission through durable intent/outcome and release.
- The read-only capability architecture guard remains green and exposes only construction plus
  its opaque instance identity.

## Task 7: Closeout

Status: complete.

- Run focused contract/scheduler/host/client/process tests.
- Run full workspace tests, non-Lab workspace tests, all-features dependency guards, Clippy,
  formatting, forbidden-source scans, and `git diff --check`.
- Perform a fresh whole-C3a review and fix every Critical or Important finding.
- Update `PLANS.md` and `CHECKPOINT.md`, commit and push each completed implementation unit,
  create a checkpoint tag, and record evidence in Issue #36.
- Do not merge into `main` or the umbrella repository.

Fresh review corrections:

- Runtime now recovers the scheduler's latest matching renew/release result before DeviceProxy
  prevalidation and retrieves the original terminal event from the ledger. Idempotency therefore
  survives loss of the bounded connection cache without duplicate state transitions or events.
- Runtime-client now separates host fatality from fallback eligibility. Only busy, cooldown, and
  explicitly transient backend failures remain fallback-eligible; fencing, identity, config, and
  protocol failures become fatal at the `InputBackend` boundary and retain their Runtime code.

Closeout result:

- Focused contract, scheduler, host, client, daemon, process, and architecture suites passed.
- Full workspace and non-Lab workspace tests passed; full-workspace Clippy passed with warnings
  denied; formatting, dependency, forbidden-source, and diff checks passed.
- The repeated whole-C3a review found no remaining Critical or Important issue after the replay
  and severity corrections.
- C3a is complete on `issue-35-runtime-ledger-v3` and is anchored by
  `checkpoint/20260711-c3a-runtime-seed`. The branch remains unmerged.
- The next seed critical-path phase is C4. C2 remains independently ready.
