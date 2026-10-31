# Player hold

Workflow #429 R-7a (register [§1.233](v012-register.md), since 0.12.35; `runtime-client`
revision 4; frozen design `rd-1031/design-r7a-hold-v3.1.md`, §2 to §6). A person takes one
instance away from the Runtime to play on its device, and gives it back. A player hold is the
instance's own state, never a scheduling pause: the pause table, the `host.scheduling_pause`
rows and ruling 116's five pauses are untouched by it. Alice's rulings 134 (no cancel during
the drain, no reserved shape for one) and 135 (every formal channel may end a hold; the
channel is only recorded in `source`) bind this contract.

**Slices.** R-7a freezes the shapes, the codes and the pure functions of the contract crate,
and the client's methods with their settlement: Runtime 0.12.35 answers both requests
`not_implemented` (Denied, recording nothing, as R-0's kinds; [runtime-interface.md](runtime-interface.md),
"Player hold request kinds") and fills none of the new members. R-7a2 (the next pull request
of the same release) moves the existing call sites to the one order of the control gates below
without a wire change. R-7b adds the state machine, the persistence, the fences on every
request and every automatic mechanism, the read surfaces, the CLI and MCP. Everything below
marked R-7b is the behaviour R-7b must build on these shapes; the shapes change only through a
new `runtime-client` revision (O-6).

## Requests and results

```text
AcquirePlayerHold { instance_alias, drain_timeout_ms }
  -> PlayerHoldAcquired { hold: PlayerHoldState, drained: { finished, cancelled } }

ReleasePlayerHold {
  instance_alias,
  expected: { owner_epoch, hold_id, revision },
  startup_package?: run | skip            // absent = run
}
  -> PlayerHoldReleased {
      instance_alias, hold_id, revision,      // revision: the slot's after the lift
      next: reconnected | instance_paused | scheduling_paused,
      selfcheck?: SchedulingResumeSelfCheck,  // only with next = reconnected
      startup_package: scheduled | cancelled | none,
      startup_package_withheld?: instance_paused | scheduling_paused,
      pauses_at_end?: { instance_revision?, global_revision? }
    }
```

Both results travel inline beside `kind` (`player_hold_acquired`, `player_hold_released`); a
receipt carrying either is `completed` (else `invalid_player_hold_receipt`). Both kinds are
general instructions, copied through G14. Success codes: `request_acquire_player_hold_completed`,
`request_release_player_hold_completed`.

### Order of the acquire's checks (fixed)

1. Shape (`RuntimeRequest::validate`): `drain_timeout_ms` in `1_000..=600_000`, else
   `invalid_player_hold_drain_timeout`.
2. Source gate: only `(User, Ui)`, `(Cli, Cli)` and `(Lab, Lab)`, else
   `invalid_player_hold_origin`. These are the envelopes of the four counters: the console is
   `(User, Ui)`; actingctl and MCP are `(Cli, Cli)`, MCP on the channel `mcp`; actinglab is
   `(Lab, Lab)`. Unlike the pause's gate it takes the Lab, because a hold holds the device, not
   dispatch. Neither request is limited by channel.
3. Instance: a registered physical instance (`instance_unknown`,
   `fixture_execution_scope_forbidden`).
4. Tags: `player_hold_not_durable` when the effective tag patterns do not keep the key.
5. Outside the hold table's lock: one ledger read by `request_id` (`EventQuery { request_id }`),
   kept for step 7; it decides "accepted in an earlier owner epoch", the request's own G14 copy
   excluded.
6. Under the hold table's lock (the record's lock too), when this `request_id` is the current
   hold's `hold_id`: `held` answers Completed with the hold, touching nothing and recording
   nothing; `requesting`, `draining`, `handing_back` refuse `player_hold_acquiring`; `releasing`
   refuses `player_hold_releasing`.
7. An id already used with no hold of it now: `player_hold_request_reused` (accepted in this
   epoch, a restored hold's `hold_id`, or step 5 found it in an earlier epoch).
8. The instance's record is `running` (the last release still reconnects): `player_hold_releasing`.
9. Another hold: `player_hold_releasing` in `releasing`, else `player_hold_already_held` (with
   `hold_id`, `hold_phase`).
10. Accepted: the record first, then the row, then the gate.

Steps 1 to 9 refuse at the entry: they write no record and replace none. A hold may be acquired
whatever pause holds the instance, in any stage.

### Order of the release's checks (fixed; the expectation before "not held", as the conditional resume)

1. Shape: revision 0, or a `hold_id` of the wrong form (it does not decode),
   `invalid_player_hold_expectation`.
2. Source gate, as the acquire's.
3. Instance, as the acquire's.
4. (Under the hold table's lock) `expected.owner_epoch` is not the current epoch:
   `player_hold_expectation_mismatch`.
5. No hold: `player_hold_not_held`.
6. `expected.hold_id` differs: `player_hold_expectation_mismatch`.
7. `expected.revision` differs: `player_hold_expectation_mismatch`.
8. Phase: the acquire's three answer `player_hold_acquiring`, `releasing` answers
   `player_hold_releasing`.
9. Accepted: the record `{release, running, phase: releasing}`, the phase `releasing`.

Steps 4 to 7 are one atomic comparison under the lock. `player_hold_expectation_mismatch`
always carries the current `current_owner_epoch`, `current_hold_id`, `current_revision`,
`current_phase` (the last three absent without a hold); with the same `hold_id` a client may send
again with the new values without asking the person, otherwise it asks again. The method never
resends by itself. Any formal channel may end a hold another channel began (ruling 135); who did
is the record's `source`.

### Release: which branch (read the pauses twice)

1. Under the persistence gate, read the pauses now. A pause holds the instance: the paused
   branch, touching no device.
2. No pause: let go of the gate and wait outside it at most 3 s for the ResumeReconnect claim; no
   claim: `lease_queue_expired`, the hold back to `held`.
3. With the claim, take the gate again and read the pauses again: still none, write
   `host.idle_state`, write the row without the hold, open the gate, reconnect, self-check,
   `next: reconnected`; a pause now (it came during the wait), the paused branch under the same
   gate, then release the claim without reconnecting.

`next` is the first pause in effect by the order below: both pauses give `instance_paused`.
`pauses_at_end` records the revision of each pause in effect at that moment. A fence or a
blackout does not change `next`.

### The answer's shape self-check

`PlayerHoldReleased` and a completed release record's `result` follow one group
(`invalid_player_hold_receipt` on the answer, `invalid_player_hold_state` on the record):

| `next` | must carry | must not carry |
|---|---|---|
| `reconnected` | `selfcheck` | `pauses_at_end`, `startup_package_withheld` |
| `instance_paused` | `pauses_at_end.instance_revision` (`global_revision` either way) | `selfcheck` |
| `scheduling_paused` | `pauses_at_end.global_revision` | `pauses_at_end.instance_revision`, `selfcheck` |

`startup_package_withheld` comes only beside `startup_package: scheduled` and equals `next`;
`pauses_at_end` holds at least one revision and appears only when `next` is a pause.

### The startup package and the idle record

- `run` (the default) with a configured package: on `reconnected` the reconnect's key goes to a
  new startup package claim (`scheduled`), replacing a queued one; under a pause the new claim is
  queued behind it (the W2-B row in `host.idle_state`) and runs after the resume's reconnect
  (`scheduled` with `startup_package_withheld`).
- `skip`: cancels the instance's queued startup package claim (`scheduler.denied
  lease_queue_cancelled`) and ends its W2-B row: `cancelled`, or `none` when there was none.
- No package configured: `none`.
- A release also ends the instance's idle row (`action`, `wake_at_unix_ms`, `woken`).
  `host.idle_state` is written before `host.player_hold`.

## Phases

| Phase | Gate | What the Runtime does |
|---|---|---|
| `requesting` | closed | closes the gate, ends the queued operator and client entries, waits for policy admissions that read the gate open |
| `draining` | closed | drains in-flight runs; past the drain deadline they stop at their next checkpoint |
| `handing_back` | closed | waits until the device is idle, then always closes its session (inside the multi-Nemu gate too); closing the input resets maatouch and minitouch first |
| `held` | closed | touches nothing; persisted |
| `releasing` | closed; only this release's reconnect claim passes | takes the reconnect claim, lifts the hold, reconnects or touches nothing by the pauses now |

- The acquire answers only once `held`; the hand-back does not wait for the client's
  `SafeReset`.
- **Deadlines.** At acceptance one `RuntimeClockSample` S fixes `since_unix_ms` = S.unix_ms,
  `drain_deadline_unix_ms` = S.unix_ms + `drain_timeout_ms` and `grace_deadline_unix_ms` =
  `drain_deadline_unix_ms` + 30 000 (`SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS`); the waits compare
  S's monotonic reading. Status reports the stored values, never recomputed. Every wait of
  `draining` and `handing_back` ends at the grace deadline. The close starts only before it;
  once started it is never cut, and the hold carries `close_started_unix_ms` (one sample, then
  unchanged) until it leaves `handing_back`. Past the grace deadline in `handing_back` with
  `close_started_unix_ms` the close is under way (no total bound: a close that never returns
  needs a Runtime restart); without it the state is transient before `player_hold_handback_busy`.
  After the close began there are four exits: `held`; an unconfirmed close (the #403 fence stays,
  on this instance only); the `reached_held` row refused (`player_hold_persist_failed`); a
  shutdown (`player_hold_interrupted`).
- After the release's row is written the hold is gone from Status while the record is
  `{release, running, phase: releasing}` until the reconnect, the self-check and the key's hand-over
  end.
- "Unconfirmed" is never a Runtime phase: it is the client's state when a receipt was lost.

### Transitions and failure exits (R-7b)

| From | Event | To | Row | Record | Receipt |
|---|---|---|---|---|---|
| none | acquire accepted | `requesting` | record, then the row (no `reached_held_unix_ms`), then the gate | `{acquire, running, requesting}` | — |
| none | that row refused | none | — | `failed`, `player_hold_persist_failed`, `requesting` | Failed |
| `handing_back` | idle, session closed, or the close confirmed with an error (ISO-2) | `held` | the row again with `reached_held_unix_ms` | `completed`, `result.drained` | `PlayerHoldAcquired` |
| `draining` | a run not at a checkpoint at the grace deadline | none | the row without the hold, then the gate open | `failed`, `player_hold_drain_incomplete`, `draining` | Failed |
| `handing_back` | a lease, a worker's policy run, an autostart key or a ladder step not ended, or the close not started, at the grace deadline | none | as above | `failed`, `player_hold_handback_busy`, `handing_back` | Failed |
| `handing_back` | the close unconfirmed | none | as above; the fence stays | `failed`, the fence's code, `handing_back` | Failed |
| `handing_back` | the `reached_held` row refused | none | the row without the hold (refused again: the Runtime is poisoned) | `failed`, `player_hold_persist_failed`, `handing_back` | Failed |
| acquiring | Runtime shutdown | kept, `held` after the next start | unchanged | gone with memory | Failed, projection `runtime_unavailable`, `player_hold_interrupted` |
| `held` | release accepted | `releasing` | — | `{release, running, releasing}` | — |
| `releasing` | no pause, claim within 3 s, still no pause | none | `host.idle_state`, the row without the hold, the gate open | `running` | — |
| (then) | reconnected, self-checked, key handed over or released | — | — | `completed`, `next: reconnected` | `PlayerHoldReleased` |
| `releasing` | no pause, no claim within 3 s | `held` | unchanged | `failed`, `lease_queue_expired`, `releasing` | refused `lease_queue_expired` |
| `releasing` | a pause (at the first or the second read) | none | `host.idle_state`, the row without the hold, the gate open, no device | `completed`, `next` a pause, `pauses_at_end` | `PlayerHoldReleased` |
| `releasing` | `host.idle_state` or the row refused | `held` | the hold unchanged | `failed`, `player_hold_persist_failed`, `releasing` | Failed; the claim released, the gate closed |
| any | an append failure (not a refusal) | — | — | — | the Runtime is poisoned, as for pauses |

A failure lifts only this hold: pauses and fences stay. No half-open hold is left: an acquire
failure opens the gate, a release failure leaves the hold `held` with its gate closed. A failure
after the reconnect does not roll the release back (`result.selfcheck` names it). Every final
state is recorded before its receipt.

## Identity

| Member | Meaning | Lifetime |
|---|---|---|
| `hold_id` | the `request_id` of the acquire; known to the client before it submits | across restarts |
| `revision` | the instance's hold slot: one more at each set and each lift, not on a phase change | from 0 in each owner epoch; a restored hold is 1 |
| `owner_epoch` | Status' top-level epoch | new at each start |
| `source` | `{actor: user \| cli \| lab, channel: ui \| cli \| mcp \| lab, via?: mcp \| ui}`; `actor` from the envelope, `channel` and `via` from the connection's binding. `channel` always present; `via` only on `lab` | across restarts |
| `since_unix_ms` | when the acquire was accepted | across restarts |
| `reached_held_unix_ms` | when the hand-back finished | persisted; none on a hold restored from a stop during its acquire |
| `close_started_unix_ms` | when the close began | `handing_back` only; not persisted |
| `restored_from_owner_epoch`, `restored_from_phase` (`acquiring` \| `held`) | on a restored hold only; `held` exactly when the row had `reached_held_unix_ms` | Status only |

The channel only says who began or ended; it decides nothing (ruling 135).

## Status members (`runtime-client` 4)

| Where | Member | Content |
|---|---|---|
| `RuntimeInstanceStatus` | `player_hold?` | `PlayerHoldState` (the members above, plus `phase`, `drain_deadline_unix_ms?`, `grace_deadline_unix_ms?`) |
| `RuntimeInstanceStatus` | `player_hold_operation?` | the instance's last accepted operation of this epoch |
| `RuntimeInstanceStatus` | `control?` | `{code, dispatch: [instance_gate], device: [instance_gate]}`, derived only by the contract crate |
| `RuntimeStartReport` | `player_holds_restored`, `player_holds_dropped`, `player_hold_not_durable` | absent when empty or `false`; the lists hold valid aliases, each once |

**The hold's self-check** (`invalid_player_hold_state`): the two deadlines in the acquire's three
phases only; `close_started_unix_ms` only in `handing_back`; `reached_held_unix_ms` never while
acquiring, and on `held` and `releasing` unless `restored_from_phase` is `acquiring` (which never
has it); the two `restored_*` together; `via` only on `lab`; `revision` and `since_unix_ms` not 0.
A hold or a record without `channel` does not decode.

### The operation record

```text
player_hold_operation? = {
  request_id, kind: acquire | release, state: running | completed | failed,
  phase?,                          // running: an acquire's hold phase, a release's releasing
  hold_id,                         // an acquire's own request_id; a release's expected.hold_id
  source: { actor, channel, via? },
  since_unix_ms, ended_unix_ms?,   // ended only in a final state
  code?, failed_phase?,            // both when failed
  result?: { revision?, drained?, next?, selfcheck?, startup_package?,
             startup_package_withheld?, pauses_at_end? }   // when completed
}
```

Self-check (`invalid_player_hold_state`): `running` has `phase` and none of `ended_unix_ms`,
`code`, `failed_phase`; `failed` has `ended_unix_ms`, `code`, `failed_phase`; `completed` has
`ended_unix_ms` and `result` and no `code`; a completed release's `result` follows the answer's
group above; an acquire's result carries no release member; `source` as the hold's.

Rules (R-7b):

- **H-1** Accepting records first: an operation is recorded once it passed its entry checks,
  before any state change, persistence or receipt.
- **H-2** An entry refusal neither writes nor replaces the record (the idempotent Completed of
  acquire step 6 included), so another counter's refused request never pushes out a `running`
  record.
- **H-3** A `running` record ends only by its own final state; one instance has at most one
  `running` record (ruling 134: no exception).
- **H-4** In one epoch every end of a hold and every failed acquire leaves a record; a record is
  only replaced, never deleted. It stays after the hold is gone and is cleared at the next start.
- **H-5** Every refusal of either request before the record is a receipt, never a dropped
  connection; only a host error of its own ends the connection (`requests.rs`).
- **H-6** The host handles a connection's requests synchronously. A failed receipt write also ends
  the connection, after the request was handled and recorded (H-1). The request cache is per
  connection: the same request again on the same connection gets the stored receipt; the same id
  with other content ends the connection unhandled. The methods never resend, on the same
  connection or a new one.

Final-state moments: an acquire turns `completed` after the `reached_held` row, before the
receipt, and `failed` after the gate opened; a reconnecting release turns `completed` after the
reconnect, the self-check and the key's hand-over (not when the receipt is written); a paused
release at once after the lift; a release whose persistence was refused `failed` once the hold is
`held` again.

## The one order of the control gates (normative; cite as "R-7a §2.6")

**O-1.** The control gates in effect on an instance, in this order; the first is the effective
gate and its code the effective code:

| # | Gate (`instance_gate`) | Effective code (`control_gate_code`) | Scope | In effect |
|---|---|---|---|---|
| 1 | `player_hold` | `dispatch_player_held` (info) | instance | the hold in any phase |
| 2 | `instance_pause` | `dispatch_paused_instance` (info) | instance | its pause in any stage |
| 3 | `global_pause` | `dispatch_paused_global` (info) | every instance | the global pause |
| 4 | `resource_fence` | `dispatch_resource_fenced` (info) | instance | its #403 fence in any state |

The effective code is the reason everywhere a dispatch does not happen: the evaluation's
deferral, the admission's refusal, Status, ScheduleExplanation and the not-run history.
`dispatch_resource_fenced` is for dispatch only: device requests refused by a fence and the
fence's own records keep `instance_resources_fenced` (error / environment_fault).
`control_gate_code` is the contract's typed spelling of the four runtime-host codes; the host
emits the code.

**O-2.** After the control gates come the positions 2 onwards of
[scheduling-blackout.md](scheduling-blackout.md), "Order of the deferrals", unchanged: its
position 1 is split into 1a to 1d, the rows above.

**O-3.** One function writes the order, `ControlGates::evaluate(ControlInputs {player_hold,
instance_pause, global_pause, resource_fence}) -> ControlGates {dispatch, device}`, with
`ControlGates::code()` the first dispatch gate's code. `device` holds only `player_hold`. Every
Runtime call site calls it (R-7a2); clients call `instance_gates` (O-7 4).

**O-4.** The four inputs of one decision are read at one moment: the evaluation's snapshot at the
start of the cycle (where the pause snapshot is taken today), under the admission lock at
admission, and when Status is assembled.

**O-5.** A change of the effective code is a change of reason (ScheduleExplanation's
`since_unix_ms` restarts); no change of the code changes nothing: adding or lifting the global
pause over ruling 116's instance pauses keeps `dispatch_paused_instance`.

**O-6.** A later control gate is added only to O-1's table and to O-3's function, as a new
`runtime-client` revision first (`instance_gate` is closed and `control` refuses an unknown value,
so the readers go first), and filled by the next slice. Since Workflow 114-A (register §1.236)
the ledger keeps its own copies of `instance_gate` and of the hold's vocabularies (a
`policy.not_run_changed` record's `effective` and hold members, word for word R-7a's `ALL` by
its guard test), so a word added to any of them also raises the `ledger` revision in the same
release ([component-interfaces.md](component-interfaces.md), `ledger` 5).

**O-7. `control` is derived by the contract crate, never written.**

1. No method sets `RuntimeInstanceStatus.control`; it comes only from decoding and from the
   derivation.
2. `RuntimeControlPlaneStatus::with_derived_control()` fills each instance's `control` from the
   same status's four members (its hold, its pause, the top-level global pause, its fence), absent
   with no gate, then validates. It is the last step of assembling a status; calling it again
   changes nothing. `with_scheduling_pause`, the one method that changes a member afterwards,
   derives again: the status remembers the derivation in a flag that is not on the wire and not
   part of equality, and also derives again when an instance already carries a `control` (a
   status read in), so the order of the calls never poisons the Runtime.
3. R-7a's host does not call it (no `control` on the wire); R-7b's host calls it at the end of the
   status assembly.
4. Readers do not read `control`: `RuntimeControlPlaneStatus::instance_gates(alias)` evaluates the
   members, so an R-7a Runtime's status (pauses, no `control`) reads correctly. CLI and MCP JSON
   carry `control` for readers without the contract.
5. `RuntimeControlPlaneStatus::validate` checks a `control` that is present: it equals what the
   members derive and has a dispatch gate, else `invalid_runtime_instance_control`. An absent
   `control` is not checked. The check is at the top level because the global pause is a
   top-level member.

**Two behaviour changes R-7a2 makes** (register §1.234, since 0.12.35). `deferral_code` put the
global pause first before it (R-7a2 removes it); the order puts the instance pause first, so with
both pauses the deferral and the admission refusal become `dispatch_paused_instance`. The fence
enters the evaluation at position 1d (`dispatch_resource_fenced`, no wake) instead of being
selected and refused at admission; the admission's check stays for intents evaluated before the
fence, with that code. A candidate reaches position 1 only past its placement: on an instance
whose device the host self-checks, the fence also withholds its policy availability (#403), so
its candidates stop at placement with `instance_unavailable`, as before R-7a2. Readers that read
not-run reasons treat the four control codes as one class by `instance_gate`, never by one code.

**"The hold ended, automatic scheduling stays paused".**
`RuntimeControlPlaneStatus::hold_ended_with_pause(alias)` is true exactly when the instance's
record of this epoch is `{release, completed}`, its `result.next` is a pause, and a pause it saw at
the end is still the same one: `pauses_at_end.instance_revision` equals the instance pause's
`revision`, or `pauses_at_end.global_revision` the global pause's. A pause lifted and set again has
another revision; the record is gone after a restart.

## Lost receipts

The rule: read the original operation before deciding to send anything; when unsure, never send
blind.

### The client (`runtime-client`)

```text
prepare_acquire_player_hold(alias, drain_timeout_ms) -> PreparedPlayerHoldAcquire
prepare_release_player_hold(alias, expected, startup) -> PreparedPlayerHoldRelease
    .request_id(), .owner_epoch(), .attempt(trigger) -> PlayerHoldAttempt
submit_acquire_player_hold(prepared) -> PlayerHoldAcquired
submit_release_player_hold(prepared) -> PlayerHoldReleased
acquire_player_hold(...), release_player_hold(...)   // prepare, then submit
```

- Prepare writes nothing; submit writes once, only on the handle that prepared it
  (`runtime_player_hold_prepared_client_mismatch` otherwise, nothing sent).
- A console stores `attempt(reopened)` (every member public, serializable, unknown members
  refused) before it submits, and settles with it after it reopened.
- **Receipt waits.** The socket read timeout is the whole formula, the receipt-read deadline the
  formula without the IO margin, cut to the session's deadline when that comes first:
  - acquire: `drain_timeout_ms + 30 000 + PLAYER_HOLD_CLOSE_ALLOWANCE_MS (30 000, until R-7b
    measures the close) + io`;
  - release: `3 000 + 30 000 + io`, as an instance resume.

  So a socket timeout always finds the deadline passed: `runtime_receipt_timeout`. A connection
  that ends before the deadline is `runtime_receipt_header_failed` with end of stream, reset or
  abort (dropped); one that ends after it counts as the timeout. A timeout latches the
  connection as every receipt timeout does, until the settlement replaces it.

**When it settles,** after the frame was written:

| What happened | `trigger` | Who settles |
|---|---|---|
| the connection ended before the receipt header (`dropped_before_receipt`) | `dropped` | the method |
| the receipt-read deadline passed (`runtime_receipt_timeout`) | `timed_out` | the method |
| any other failure after sending, or the process that submitted is gone | `reopened` | the caller, with `settle_player_hold` after reading Status |

A real receipt is never settled: `player_hold_interrupted` (Failed, projection
`runtime_unavailable`, not uncertain) is returned as it is; the stored attempt settles A11 after
the restart. `settle_player_hold(Some(attempt), ...)` is called only inside the method, after
submit returned a failure, or once the submitting process is gone; while a submit is still on
its way in the same process only with `None`.

**How the method settles:** close the timed-out connection; read the runtime info again; in the
same owner epoch put a new connection into the handle (the same channel) as
`reconnect_in_owner_epoch` does; in another epoch read Status through a temporary client of the
same configuration and latch the handle `runtime_owner_epoch_changed`; read Status once; call
`settle_player_hold`; return `runtime_player_hold_settled`, `runtime_player_hold_unconfirmed` or
`runtime_operation_unsupported`, each carrying `player_hold_settlement()` and
`player_hold_attempt()`, the original failure as `related` (and the epoch change when there was
one). It reads once, never polls, never resends.

| Afterwards | The handle | The method returns |
|---|---|---|
| same epoch, the new connection's handshake succeeded | the new connection in place, usable | a code of the table |
| another epoch | latched `runtime_owner_epoch_changed` | P2, A11 to A13, R10, R11 |
| runtime info, connect, handshake or Status unreadable | latched with that failure | `unconfirmed`, `status_unreadable` (P1) |

| Code | `is_fatal()` | uncertain | `disposition()` | Read it by |
|---|---|---|---|---|
| `runtime_player_hold_settled` (warning) | true | no (it settles without a receipt) | `runtime` | `settlement_outcome`, and `player_hold_settlement()` |
| `runtime_player_hold_unconfirmed` (error) | true | yes | `uncertain` | `settlement_observed`, `settlement_final` |

Both are carrier codes: never worded by the class's general sentence.

### The settlement table (`settle_player_hold`)

```text
settle_player_hold(attempt: Option<&PlayerHoldAttempt>, alias, status: Option<&RuntimeControlPlaneStatus>)
  -> PlayerHoldSettlement = Current(PlayerHoldView) | Decided(PlayerHoldDecision)
PlayerHoldAttempt { kind, request_id (R), sent_in_owner_epoch (E0), expected_hold_id (H), trigger }
PlayerHoldView    { alias, hold?, operation?, gates, hold_ended_with_pause }
PlayerHoldDecision { rule, verdict, final, settlement_outcome?, observed?, restored, attempt?,
                     status_owner_epoch? (E1), hold?, operation?, ended_by?: {source, state} }
```

Without an attempt the function describes the instance now. **Own** means `rec.request_id = R`;
anything else, no record included, is not own. **Possibly late**: `timed_out` or `reopened`.
**Revision-4 sign**: any instance of the status with `player_hold`, `player_hold_operation` or
`control`. Settled and unsupported are final; of the unconfirmed only `in_progress`,
`in_progress_other` and `status_unreadable` are not. A final A9, A10 or R6 may still be settled
again: when a later status of the same epoch shows `rec.request_id = R` (an acquire also
`hold.hold_id = R`), settle the same attempt again. An attempt can be dropped once settled or
unsupported, or once settled in a new epoch. The order is fixed: the P rows, then by epoch, top
to bottom, the first match deciding.

| # | Condition | Returns | final |
|---|---|---|---|
| P1 | no status (info, connect, handshake or Status failed) | unconfirmed `status_unreadable` | no |
| P2 | the status does not list the instance | unconfirmed `instance_not_listed` | yes |

Acquire (H = R), E1 = E0:

| # | Condition | Returns | final |
|---|---|---|---|
| A1 | own, `running` | unconfirmed `in_progress` (with the phase, deadlines, `close_started_unix_ms`) | no |
| A2 | own, `completed` | settled `completed` (from `rec.result`) | yes |
| A3 | own, `failed` | settled `failed` (`code`, `failed_phase`, `drained`) | yes |
| A4 | `hold.hold_id = R` in `held` or `releasing` | settled `completed` | yes |
| A5 | `hold.hold_id = R` in the acquire's phases | unconfirmed `inconsistent` | yes |
| A6 | `rec.kind = release` and `rec.hold_id = R` | settled `completed`, `ended_by` | yes |
| A7 | no record, `dropped`, no revision-4 sign | unsupported (`runtime_operation_unsupported`) | yes |
| A8 | no record, `dropped` | settled `not_performed` | yes |
| A9 | no record (possibly late) | unconfirmed `not_registered` | yes |
| A10 | otherwise | unconfirmed `superseded` | yes |

Acquire, E1 ≠ E0:

| # | Condition | Returns | final |
|---|---|---|---|
| A11 | `hold.hold_id = R` | settled `completed`, `restored` | yes |
| A12 | `rec.kind = release` and `rec.hold_id = R` | settled `completed`, `restored`, `ended_by` | yes |
| A13 | otherwise | unconfirmed `epoch_changed` | yes |

Release (H = `expected.hold_id`), E1 = E0:

| # | Condition | Returns | final |
|---|---|---|---|
| R1 | own, `running` | unconfirmed `in_progress` | no |
| R2 | own, `completed` | settled `completed` (from `rec.result`) | yes |
| R3 | own, `failed` | settled `failed` | yes |
| R4 | `hold.hold_id = H` in `releasing` | unconfirmed `in_progress_other` | no |
| R5 | `hold.hold_id = H`, `dropped` | settled `not_performed` | yes |
| R6 | `hold.hold_id = H` (possibly late) | unconfirmed `not_registered` | yes |
| R7 | `rec.kind = release` and `rec.hold_id = H` | settled `ended_by_other`, `ended_by` | yes |
| R8 | no record, `dropped`, no revision-4 sign | unsupported | yes |
| R9 | otherwise | settled `hold_ended` | yes |

Release, E1 ≠ E0:

| # | Condition | Returns | final |
|---|---|---|---|
| R10 | `hold.hold_id = H` | settled `not_performed` | yes |
| R11 | otherwise | settled `hold_ended` | yes |

Why it holds: only a dropped connection lets "no record" mean "not performed": the host handles
a connection synchronously, so a connection it ended will not handle the request later (H-6);
a timeout or a reopened console has no such guarantee. A new epoch's process never receives the
old epoch's frame, so a hold `R` there was restored from its row, written at acceptance (A11,
A12). R9 and R11 are safe when late: H is gone, so a late release is refused at the entry. A7
and R8 mean an older Runtime: R-7a's and later Runtimes always answer both requests (H-5).
Results are returned only with a receipt; every row returns a client code, never a forged
result. R6 says only "the hold stands now, your release may still take effect"; how it is shown
is the console's.

## Fences while a hold lasts (R-7b)

"Refused" is the entry refusal `player_hold_active` (error; `alias`, `hold_id`, `hold_phase`,
`operation`) on every channel, the console's included, from `requesting` on.

| Request or mechanism | While held |
|---|---|
| `RunContainedTask`, `RunContainedLabOperation`, `AcquireLease`, `QueueLease`, `ApplicationLifecycle`, `ControlEmulatorInstance` (stop too), `SelfCheckInstance`, `SafeReset` (it still settles `client_resets`), `DrillRecoveryLadder` device | refused at the entry; the queue gate grants none of their claims |
| `Input` | refused under the admission lock, on an existing lease too |
| `ObserveReadonly`, `CaptureSequence`, `ObserveContainedPage` | refused under the admission lock (ruling 101) |
| `PollQueuedLease`, `RenewLease`, `ReleaseLease`, `CancelQueuedLease`, `CancelContainedTask`, `DrillRecoveryLadder` plan, `StopInstanceRun` (while requesting or draining), `RunInstanceTask` (`not_implemented` first) | taken |
| `PauseScheduling` | taken; every stage completes at once (`drained {0, 0}`) |
| `ResumeScheduling` global | taken; it never reconnects |
| `ResumeScheduling` instance | lifts the pause only, no claim, no reconnect; `selfcheck` both sides `ok: false`, `failure_code: player_hold_active` |
| every read, `DiscoverInstances`, every other general instruction, the Lab option-one kinds and option-two kinds that touch no device | taken |
| policy dispatch, fact refresh | `Deferred`, `dispatch_player_held`; the admission refuses again by the same order |
| startup package claim | held by the gate, kept queued; replaced (`run`) or cancelled (`skip`) by the release |
| recovery ladder | not admitted (`recovery_admission_denied`); a climbing ladder ends at its next step; the invariant `PlayerHeld` before `Paused` |
| the emulator watch, the daemon start's start, IDLE-1's `stop_emulator` wake | the one place `automatic_start_hold` answers `player_held` (with the hold's revision) before the instance's pause; skips recorded once per reason and revision |
| idle actions, IDLE-1's `close_app` wake | not done; not queued |
| the fence recheck, the monitor probe, the handset link's `Reconnect` and `Prepare`, the device state sample | skipped |
| the start preparation | skips a restored hold |
| the installation's idle check | entries the hold's gate holds count as not waiting |
| the deploy adb guard (Tools, #421) | a held instance is not probed; `skipped_player_held` |

From `held` until the release's claim, the device sees no adb call at all.

## Persistence and restart (R-7b)

- **The fact.** `host.player_hold`: Runtime scope, `record_list`, no TTL; one row per hold:
  `instance_alias`, `hold_id`, `since_unix_ms`, `source_actor`, `source_channel` (required),
  `source_via?`, `set_in_owner_epoch`, `reached_held_unix_ms?`, `restored_from_owner_epoch?`. An
  empty list is no hold. Every change happens under `scheduling_pause_persist_gate`, shared with
  the pauses; the row is written before the gate changes; an acquire writes twice, a release
  once. The pause rows are never touched.
- **Start.** Right after the pauses are restored and before the start preparation each row is
  restored `held`, revision 1, `restored_from_owner_epoch` the previous epoch, `reached_held_unix_ms`
  kept (a hold stopped during its acquire reports `restored_from_phase: acquiring` at every start
  until it ends); a row of an alias no longer registered is dropped; the restored rows are written
  again. stdout: `actingd player_hold_restored instance=<alias> hold_id=<id> since=<ms>
  from=<acquiring|held>` or `actingd player_hold_dropped instance=<alias>
  reason=instance_not_registered`. A damaged row: `player_hold_fact_invalid`, the start fails.
- **The tag.** The default `ledger_observer.tag.workbench_patterns` gains `host.player_hold` (a
  default, not a key; `actingd-config` unchanged). When the effective patterns do not match the
  whole key `host.player_hold` (`*` any text), the start reads the one index row `(state,
  runtime_facts/runtime/host.player_hold)`, taking only that exact key from the prefix read: rows
  of registered aliases there fail the start with `player_hold_fact_untagged` (`fact_key`,
  `hold_count` of registered aliases); a damaged record is `player_hold_fact_invalid`; a failed
  index read fails the start with its own code (`ledger_joint_transaction_unsupported` when the
  store keeps no index); no row, an empty list or only unregistered aliases start as usual with
  `StartReport.player_hold_not_durable` and every acquire refused `player_hold_not_durable`.
- A shutdown keeps a hold: the person already took the device, and the second round of the
  shutdown closes every session.

## Interactions

- Pauses and holds never rewrite each other; they only order under one persistence gate. While a
  hold lasts another counter may set, change or lift pauses; the release reads the pauses of that
  moment and never writes back an earlier snapshot. "Start all" lifts the global pause only.
- Ruling 116's five pauses stay under every acquire, release, failure, restart and deploy;
  ending a hold under them is `next: instance_paused`, and `hold_ended_with_pause` stays true
  while they stand. Lifting them is Alice's alone.
- The installer (#421): the guard `player_holds` lets an operation switch only to a Runtime/Tools
  build whose `player-hold` range holds 1 while any hold exists (an undeclared build counts
  [0, 0]); S0 and the post-start check carry the holds; the installer never ends a hold; its own
  `release-hold` reads an instance resume's `failure_code: player_hold_active` as expected.
- #419: every hold failure stays on its instance; only an append failure poisons, as for
  pauses. #403: a fence beside a hold waits; a release without a pause rechecks it through the
  reconnect.

## Codes, vocabularies, constants

- **runtime-host:** `player_hold_active`, `player_hold_already_held`, `player_hold_acquiring`,
  `player_hold_releasing`, `player_hold_not_held`, `player_hold_expectation_mismatch`,
  `player_hold_request_reused`, `player_hold_drain_incomplete`, `player_hold_handback_busy`,
  `player_hold_interrupted` (warning, not uncertain, a carrier code), `player_hold_persist_failed`,
  `player_hold_not_durable`, `player_hold_fact_invalid`, `player_hold_fact_untagged`,
  `player_hold_restored` (info), `player_hold_dropped` (warning), `dispatch_player_held` and
  `dispatch_resource_fenced` (info); `emulator_autostart_skip_reason` gains `player_held`.
- **runtime-client:** `runtime_player_hold_settled` (warning), `runtime_player_hold_unconfirmed`
  (error, uncertain), `runtime_player_hold_prepared_client_mismatch`.
- **contract:** `invalid_player_hold_origin`, `invalid_player_hold_drain_timeout`,
  `invalid_player_hold_expectation`, `invalid_player_hold_state`, `invalid_player_hold_receipt`,
  `invalid_runtime_instance_control`; the success codes above; the locations
  `acquire_player_hold`, `release_player_hold`.
- **Vocabularies:** `player_hold_phase`, `player_hold_next`, `instance_gate` (in O-1's order),
  `control_gate_code`, `player_hold_operation_kind`, `player_hold_operation_state`,
  `player_hold_restored_from`, `player_hold_startup_package`, `player_hold_source_actor`,
  `player_hold_settlement_trigger`, `player_hold_settlement_verdict`,
  `player_hold_settlement_outcome`, `player_hold_unconfirmed_observed` (the first three not
  final), `player_hold_settlement_rule` (`P1`, `P2`, `A1`…`A13`, `R1`…`R11`); `channel` and its
  values for `channel` and `via`.
- **Value keys** (renamed where the design's name was taken or reserved): `hold_phase` (the
  design's `phase`, which is the install phase's key), `hold_revision`, `hold_next`,
  `hold_restored`, `settlement_rule`, `settlement_trigger`, `settlement_final` (`final` is a Rust
  keyword), `settlement_outcome` (`outcome` names the envelope), `settlement_observed`, `alias`
  for an instance alias, `failure_code` for a failed operation's code, and as designed `hold_id`,
  `failed_phase`, `current_*`, `hold_count`, `restored_from_phase`, `operation_kind`,
  `startup_package`, `startup_package_withheld`, `pause_instance_revision`,
  `pause_global_revision`, `drained_*`, `selfcheck_failure_code`, `source_*`, `ended_by_*`,
  `operation_kind_now`, `operation_state_now`.
- **Constants and functions** (contract crate): `PLAYER_HOLD_CLOSE_ALLOWANCE_MS = 30 000` (no
  source bounds the close; R-7b measures it; changing it changes no wire shape);
  `player_hold_acquire_receipt_wait`, `player_hold_release_receipt_wait`,
  `player_hold_acquire_receipt_deadline`, `player_hold_release_receipt_deadline`;
  `ControlGates::evaluate`, `instance_gates`, `hold_ended_with_pause`, `settle_player_hold`,
  `with_derived_control`.
- Description changes, no code change: `contained_task_paused` (also a hold's drain),
  `recovery_admission_denied` (also a hold), `dispatch_paused_instance` and
  `dispatch_paused_global` (control codes in this order), `runtime_operation_unsupported` (also
  rows A7, R8).

No configuration key is added: the release's `startup_package` replaces the
`scheduling.takeover.startup_on_resume` of the old A.3 design, which never existed.

## Interfaces and rollout

- **`runtime-client` 4** ([component-interfaces.md](component-interfaces.md)): the two requests,
  the two results, the three status members and the three start report members. Request v4 and
  receipt v2 keep their schema strings, so only the installation can check a pairing. R-7a's
  Runtime declares [2, 4] and fills none; R-7b fills Status and raises `min` to 4. A console that
  sends a revision-4 request declares `min` 4; one that only reads revision-4 shapes may declare
  [3, 4].
- **`player-hold` 1** (new, containment): the Runtime writes `host.player_hold` and a later or
  earlier Runtime reads it. Declared from R-7b; checked only by #421's `player_holds` guard while a
  hold exists; not a rollback edge; the console does not declare it.
- **Three steps.**
  1. R-7a alone to production (Runtime [2, 4]): the update check takes `runtime-client` I ∩ Rp
     with I the newest kept actingctl, and R-7b's installer [4, 4] would find no overlap with
     [2, 3].
  2. A console repinned to R-7a's contract, declaring [3, 4] (UI-0a).
  3. Before R-7b: the console that will run with R-7b or a later Runtime is on the console
     channel's "player hold read surface" list. The list starts with UI-3d's first release build
     and is empty before it, so R-7b goes to production only after it. The criteria (the console
     channel's): it decodes revision 4; it shows each instance's hold phase, source and deadlines;
     it greys out the device entries while a hold lasts; `player_hold_active` (the resume
     `failure_code` included) has its own sentence. A rollback by acsetup is a whole-slot
     rollback to the kept slot's Runtime and console, and the kept slot (R-7a) has no hold, so
     from UI-2a acsetup only checks the console of the R-7b release (no acsetup build exists
     before UI-2a). Until UI-2a, R-7b goes to production by the `actingctl install` path, which has
     no console rollback target: the deployment record states the build number, the commit and
     the `runtime-client` declaration of the console in use, a person checks them against the
     list, and a standing rule "while R-7b runs, change the console only to one on the list" is
     recorded. Every later Runtime follows the same rule.
- A console of `max` 3 or lower reading R-7b's Status fails to decode it as soon as any instance
  carries a hold, a record or a control gate (`runtime_receipt_decode_failed`); with ruling 116's
  pauses that is the first status read. A revision-4 console meeting a Runtime of `max` 3 gets the
  frame dropped and settles A7 or R8 (`runtime_operation_unsupported`), the handle usable.
- **Rollback from R-7a to 0.12.34.** Nothing is persisted and no behaviour is undone. A console
  already on revision 4 that sends a new request to the older Runtime has its connection dropped;
  the client reconnects in the same epoch and reports `runtime_operation_unsupported` (A7, R8),
  the handle usable.
