# C3b Resident Control Plane Implementation Plan

Status: active.

## Authority and baseline

- GitHub authority: Issue #35, authored by `HS7097` and labeled `状态:已批准`.
- Frozen specification: `TASK-runtime-ledger-core-and-optional-lab-correction-v3.md`.
- Frozen specification SHA-256:
  `28273b85491b0d43aa7a7b7a7ece10db681de9df4d9100e85f9e9b086dd107a6`.
- Approved resource-authoring amendment:
  `AMENDMENT-Issue35-Lab-resource-authoring-ownership.md`.
- Amendment SHA-256:
  `de039ad910b0b8208d52b8582b260868d4bcd2b04ec2b272977890661a136b54`.
- C3b baseline: `b9e5e2f5061a3fbe637d753514c36d6d4dc77186`.
- C2 rollback tag: `checkpoint/20260711-c2-artifact-evidence`.
- Implementation branch: `issue-35-runtime-ledger-v3`.

## Goal

Grow the accepted C3a scheduler and daemon in place into the complete resident control plane. The
daemon becomes the only production owner of input and capture backends. Per-instance queues,
priority preemption, cooperative safe-yield transfer, and durable transfer facts are added without
moving game/domain behavior into the scheduler.

## Frozen C3b decisions

### Admission and queue policy

- Preserve C3a immediate acquisition as an explicit fail-if-busy policy.
- Add an explicit queued acquisition policy with closed `normal` and `high` priorities.
- Queue timeout is a bounded relative duration evaluated by the daemon monotonic clock.
- Queues are bounded per instance. Overflow, invalid timeout, cancellation, expiry, and disconnect
  are visible typed outcomes; none can silently disappear.
- Higher priority than the current holder requests preemption. Equal or lower priority remains
  queued and cannot interrupt the holder.
- Within one priority, admission order is FIFO by scheduler-owned arrival sequence. Instances are
  partitioned and do not share queue locks or capacity.
- A queued request is bound to its Runtime IPC connection and original request identity. The same
  connection may poll or cancel it; another connection cannot claim it.

### Safe-yield and transfer ordering

- Every input commit is a destructive execution section. A preemption request may be recorded
  while it runs, but the active lease remains valid until the input outcome is durably recorded.
- Transfer occurs only at an explicit safe boundary: idle authority, completed input outcome,
  explicit release, expiry cleanup, or connection cleanup.
- Scheduler transfer is prepared without mutating live authority.
- The transfer intent and `lease.transferred` authorization fact are durably appended before the
  prepared scheduler state is committed. Only after that commit can the queued client observe its
  new lease token.
- A failed transfer event append leaves the old authority unchanged. A post-durability invariant
  failure poisons the Runtime and cannot return a success receipt.
- Old lease tokens fail fencing immediately after committed transfer.

### Daemon-owned execution and capture

- Add `crates/execution-kernel` as the daemon-owned backend/session shell. C3b gives it input and
  capture session ownership; C5 later migrates recognition, drive, run, and recovery behavior into
  it.
- Backend sessions are opened lazily inside daemon worker threads and are never returned through
  public client APIs.
- Runtime host composes one execution session per registered instance. Input and capture failures
  are explicit and ledger-visible; there is no reconnect, retry, backend fallback, or fake result.
- Replace the C3a client-side begin/finish capture capability flow with one daemon-side
  `observe_readonly` operation. The client receives typed observation metadata, never a backend or
  writable/capturing capability.
- Remove production `actingcommand-device` and recognition dependencies from `runtime-client`.
- Actingd configuration requires explicit input and capture backend selections. Automatic backend
  selection is rejected at the process boundary.

## Implementation tasks

### Task 1: C3b contract and scheduler core

Status: complete.

- Add closed queue priority/policy, queued admission, poll/cancel, and receipt types.
- Add typed scheduler queue/preemption and lease-transfer facts with strict serde coverage.
- Extend the C3a scheduler in place with bounded per-instance queues, FIFO ordering, cancellation,
  expiry, disconnect cleanup, destructive-section state, prepared transfer, and commit fencing.
- Add adversarial unit tests for queue overflow, deadline expiry, cross-connection claim,
  deterministic competition, priority ordering, destructive deferral, stale-token rejection, and
  cross-instance independence.

### Task 2: Daemon-owned execution backend shell

Status: active.

- Add `crates/execution-kernel` and its daemon-only provider/session interfaces.
- Move backend worker/session ownership out of Runtime client reach.
- Hold input and capture backends inside daemon workers and expose only typed input/capture methods
  to Runtime host.
- Add explicit close, panic, provider-open, input, capture, and response-loss tests.

### Task 3: Runtime host control-plane integration

Status: pending.

- Integrate queue/poll/cancel and safe-yield transfer into Runtime host.
- Record every queue, preemption, transfer, cancellation, expiry, release, and denial in the global
  ledger with one correlation chain.
- Keep destructive input active through its durable outcome, then evaluate prepared transfer.
- Promote queued work on safe release/expiry/disconnect boundaries without exposing backend
  authority.
- Preserve second-daemon owner rejection/takeover and owner-epoch fencing.

### Task 4: Capture hard gate, client, and process acceptance

Status: pending.

- Execute read-only capture inside the daemon and remove the transitional client capability path.
- Update actingd typed configuration and backend registry for explicit capture selection.
- Update runtime-client and actingctl to use only the typed IPC contract.
- Add sealed and process tests for deterministic same-instance competition, independent instances,
  queued polling/cancellation, destructive preemption deferral, durable transfer order, stale-token
  rejection, daemon-owned capture, reconnecting clients, and second-daemon conflict/takeover.
- Add dependency/source guards proving production clients cannot construct input or capture
  backends.

### Task 5: Closeout

Status: pending.

- Run focused contract, scheduler, execution-kernel, host, client, process, architecture, and
  protocol suites.
- Run full workspace and non-Lab tests, formatting, all-target Clippy with warnings denied,
  all-features checks, dependency/source guards, and `git diff --check`.
- Update `PLANS.md` and `CHECKPOINT.md`, push completed units, create a stable C3b checkpoint tag,
  and record evidence in Issue #36.
- Do not merge this branch into `main` or the umbrella repository.

## Explicit non-goals

- No game logic, task package execution, recognition orchestration, recovery graph, UI, remote API,
  SQLite, resource authoring, conversion, or package publication.
- No task may directly invoke another task.
- No client-side production input or capture backend.
- No automatic retry, reconnect, backend fallback, fake success, or silent queue loss.
- No resource repository, emulator, or live-device operation is needed for C3b.

## Acceptance criteria

1. Two simultaneous same-instance clients receive deterministic authority: one active and the
   other explicitly queued, preempting, or denied according to the declared policy.
2. Different instances admit and execute independently.
3. Higher-priority preemption cannot interrupt a destructive input section and transfers only at
   the next safe boundary.
4. `lease.transferred` is durable before the queued holder can observe or use its new token.
5. Old tokens and cross-connection queue claims are rejected without backend invocation.
6. Queue overflow, expiry, cancellation, disconnect, transfer failure, backend failure, and
   capture failure are explicit ledger-visible outcomes.
7. Runtime host is the sole production owner of input and capture backends; runtime-client and
   user clients have no device dependency or backend constructor path.
8. Read-only observation captures inside the daemon and returns typed metadata without exporting
   backend authority.
9. Second-daemon conflict/takeover, fresh owner epoch, cooldown, and stale-token fencing remain
   covered.
10. Full workspace, non-Lab, formatting, all-target Clippy, all-features, architecture, dependency,
    and process gates pass.
