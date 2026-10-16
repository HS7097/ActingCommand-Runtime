# Ledger observer (Workflow #383)

This contract states the target shape of the ledger after Workflow #383: the ledger stops being a
gate in front of business writes and becomes an observer beside them. It is written by the
documentation slice AO-0 from the frozen model "账本观测者化" v1.4.1 of board issue #383
(`rd-1009/arch/model-ledger-observer.md` in the coordinator's workspace, cited below as LEDG §n). The code
slices AO-1 to AO-9 implement it; every section names its slice.

**Status.** Until the slice a section names has merged, `main` behaves as the older contracts say
(`ledger-store.md`, `instance-fact-store.md`, `runtime-fact-store.md`). Their superseded
paragraphs carry a note that points here, and the implementing slice deletes the old paragraph in
the same PR that changes the code.

**Later rulings applied** (they override the frozen model where the two differ):

1. *No package digests* (Alice 10-11, option 丙, Workflow #336). No shape, key, code or contract
   text below computes, pins or compares a task-package content digest. The ledger event hash
   chain, download SHA-256 checks of release assets and the screenshot self-check stay.
2. *No old-version compatibility* (Alice 10-11). The v0.12.0 ledger is a new ledger (revision 3).
   Nothing here reads a ledger, a state root or a configuration written before v0.12.0. What
   must hold is that a build reads back exactly what it writes itself, and, within the 0.12
   series, what an earlier 0.12 build wrote on the same state root: a 0.12.x deploy swaps the
   programs on the same root and keeps the ledger (Workflow `rd-1010/plan-v012/order-v012.md`
   R4, §3.6), and every 0.12 shape is registered once in the [v0.12.0 register](v012-register.md)
   (R5). That is one ledger format, not old-version compatibility, which means reading a ledger
   written before v0.12.0; the one-time seeding of §11.6 reads such a same-series ledger. LEDG §11
   "旧账本照旧可读" and the test rows AO6-n and AO9-d ("an older release's state root opens") are
   void.
3. *No integration branch* (Workflow `rd-1010/plan-v012/order-v012.md` §10). Slices merge to
   `main` in stack order. `main` may be unable to start actingd between slices only when the
   stage is registered in REG-0 (`ci/known-unbootable.toml`).
4. *Neutral core*. Nothing here names a game, a page or a device.

Related documents written by AO-0: [v0.12.0 register](v012-register.md) (every new persisted
shape, every removal, every configuration key and its slice) and the capacity ledger in
[Instance fact store](instance-fact-store.md#capacity-ledger-workflow-383-ao-0).

## 1 Roles

- **Two memory stores hold the live state** (LEDG §3.1):
  - the *instance fact store* (`InstanceFactStore`): observations of the application under
    automation and the instructions a person or an agent sets (`session.*`); at most 256 active
    facts for the whole host, shared by every instance, refused (never evicted) when full;
  - the *scheduler workbench*: the runtime fact store (`RuntimeFactStore`, at most 16 384 records)
    plus the scheduler's decision memory: the dispatch clock, the per-dispatch-pair
    `PolicyControlState` (daily and window counts, window runtime, failure streaks and pauses,
    retry rounds, condition reruns), the per-(instance, activity profile) activity state (daily
    and window counts, window runtime, next eligible time), the settlement idempotency cache
    (memory only, never tagged) and the agent wake obligations.
- **The ledger observes.** It is the only authoritative record for every reader outside the
  Runtime, and the authoritative history inside it; the live state belongs to the two stores
  (Alice 10-09, rule 12). It writes only what has taken effect; an intent is a separate record
  that says the Runtime decided to act; a missed stretch is written as a gap record (§10).
- **Business never waits for the ledger.** A store applies a change first; a silent gate on the
  same path copies the change into the capture queue; the ledger flushes asynchronously.
- **Direct writes** exist only where no probe can see the record: the two offline subcommands of
  the exception table (§6). Inside a running actingd nothing writes the ledger directly; start,
  takeover, close, fatal and install are copied by gates (rule 14).

## 2 Dependency direction and the four host calls (AO-2 provides; AO-3 guards)

Inside a running actingd, `runtime-host` calls the ledger in exactly four ways (LEDG §4.8):

| # | Call | Who holds it | Where it may appear |
|---|---|---|---|
| 1 | **Capture interface**: a trait in `actingcommand-contract`, implemented by `crates/ledger`, handed to the host at start | only the host function `capture` | every gate calls `capture` |
| 2 | **Flush barrier**: wait until one captured record is committed; writes nothing | the three call sites | before business opens at start (§12 step 10); the receipt of an install control (G3 `host.install_transition`, G15 release/state/catalog transitions); before the exception read X6 |
| 3 | **Exception reads** X1–X4, X6 (§11.6) and the view functions (material read) | their call sites | startup, request idempotency across restarts, material read |
| 4 | **Watermark tier read**: the capture queue's current tier (0 below tier 1, then 1–3), lock-free, no I/O | only the performance controller's pressure read (`ledger_backlog`) | added by PERF slice P-s, ships with or after AO-6, `mode` stays `observe` |

- The flush barrier and every wait for the ledger happen outside `fact_write_gate` and outside the
  `policy` lock (LEDG §3.5). Inside a gate nothing waits for the disk.
- **Architecture guard** (AO-3; test AO3-j, AO3-k): over the non-test code of `runtime-host` and
  `actingd`, the ledger write API (`append*`, `execute_critical`, `reconcile_*`, the maintenance
  writer's `import`) appears only at whitelisted sites; the flush barrier only at the three sites
  above; `query` and `latest_sequence` only in X1–X4, X6 and the view functions; the capture
  handle only in `capture`; the tier handle only in the performance controller.
  - The whitelist starts (AO-3) as every site of Appendix A and only shrinks. After AO-4 it equals
    W1 and W2 of the exception table, the three barrier sites, the exception reads, the tier read
    and the three "ledger-own triggers" (start calls that make the ledger draft its own records:
    prior-epoch closes and startup eviction recovery; Appendix A.8, M3). The guard keys on the
    receiver (`GlobalLedger`, `LedgerMaintenance`, the capture and barrier traits), not on name
    patterns (A.8, M8).
  - The exception table *is* the write part of the whitelist, row for row. Adding a row is the
    coordinator's decision and is reported to Alice; a write call that is not in the table fails
    the guard.
- **B-class payload mapping** (kernel trace to `task.*`, capture to `capture.*`, …) stays in an
  adapter beside each choke point. The adapter converts types only and calls no ledger API; the
  ledger crate never depends on `execution-kernel` (LEDG §4.8, Q1a).

### 2.1 Interface shapes

The semantics below are the contract; the Rust names are the intended shape and the implementing
slice may refine spelling without changing meaning. The traits live in `actingcommand-contract`.
If the architecture guard keeps behaviour out of the contract crate, AO-2 moves them into a new
small crate and says so (LEDG §17).

```rust
/// One business event as the gate sees it: an existing contract payload type and its links.
/// Draft assembly, sanitizing, category, ordering, watermarks, flushing and tagging all happen
/// on the ledger side.
pub struct CaptureItem {
    pub payload: EventPayloadDraft,
    pub links: EventLinksDraft,
    pub origin: EventOrigin,
    /// Set by a registered revisioned source (§8.3): the store that applied the change.
    pub source: Option<SourceRevision>,
}

pub struct SourceRevision {
    pub source: LedgerSource,   // instance_facts | runtime_facts | decision_memory | fact_refresh_transitions
    pub lineage: Lineage,       // a fresh identity per process start (per owner epoch)
    pub revision: u64,          // +1 per accepted change of that source, contiguous in a lineage
}

/// Issued at capture, before anything is written: the event id may be linked at once;
/// `capture_seq` orders the record; there is no ledger sequence yet.
pub struct CaptureTicket {
    pub event_id: EventId,
    pub capture_seq: u64,
}

pub trait LedgerCapture: Send + Sync {
    /// The state-document half of a joint capture (G15); opaque to the host.
    type JointWork: Send + 'static;
    /// Never blocks, never fails, never drops (§7).
    fn capture(&self, item: CaptureItem) -> CaptureTicket;
    /// G15: the record and its state-document work commit in one transaction on the flush thread.
    fn capture_joint(&self, item: CaptureItem, work: Self::JointWork) -> CaptureTicket;
}

pub trait LedgerFlushBarrier: Send + Sync {
    /// Returns once the record is committed. The only other ending is the process stopping
    /// fatally (§9.2): a caller never receives success for an uncommitted record.
    fn wait_committed(&self, ticket: &CaptureTicket) -> CommittedRecord;   // {event_id, sequence}
}

pub trait LedgerBacklogTier: Send + Sync {
    fn tier(&self) -> CaptureTier;   // below_tier1 | tier1 | tier2 | tier3
}
```

As built by AO-2 (register §1.38): the traits live in `actingcommand-contract`
(`ledger_capture.rs`); `SourceRevision.lineage` is the owner epoch. `CaptureItem` also carries
the record's `event_id` (issued by the caller, so it can be linked before the record exists) and
its `severity`. `wait_committed` returns `Result<CommittedRecord, Outcome>`. It returns once the
record and every record captured before it are finished, writes nothing, and then answers the
record's ledger sequence, or `ledger_record_rejected` (with `event_id`) for a record the flush
replaced by its `record_rejected` gap, `ledger_write_failed` (`attempts` 0) once the writer is
sealed (§9.2: the process is stopping fatally), `writer_unavailable` once the writer is closed,
and `writer_response_timeout` when the writer does not answer the lookup. The ledger's
implementation is `GlobalLedger::capture_handle`, whose `JointWork` is the ledger's
`Box<dyn LedgerTransactionWork>`; `GlobalLedger::capture_status` reads the tier, both segments'
records and bytes and the last capture number.

## 3 The fan-out point and the `observe_trigger` invariant (AO-3)

`emit` replaces today's `append_event_raw_with_observation` (`host/facts.rs:511-557`):

1. take `fact_write_gate` (lock order unchanged: `policy_outcome_gate` → `policy` →
   `fact_write_gate` → facts);
2. the business change is already applied by its own entry (instance store, workbench, decision
   memory), its revision +1;
3. `capture`: first the instance store's `observe_trigger` (record the input boundary, plan the
   `invalidate_on` invalidations and apply them at once), then hand the event and the
   invalidations it caused, in that order, to the capture interface; no wait;
4. release the gate; send the business notification (§13); the performance pipeline observes as
   today.

**Invariant** (LEDG §3.3): every recorded event, from a gate or not, passes `observe_trigger` at
capture time and in capture order. The host has exactly one `capture` function; the architecture
guard keeps it that way. A capture outside the gate (capture pipeline, performance, kernel trace)
first checks a small set "event types some fact currently waits on" that the instance store
replaces atomically on change; only on a hit does it take the gate and call `observe_trigger`.
The settlement continuation inside the ledger (`global.rs:1599`, called through
`policy_outcome.rs:52`, `:1203`, `:1495`) returns its records to the host, which feeds them
through `capture`'s `observe_trigger` step until AO-4 retires the continuation.

**Event types a fact may not name in `invalidate_on`** (validator `FactRecord::validate`, today
`fact.rs:321-331` refuses only the first two). These records never pass `observe_trigger`: the
offline subcommands have no instance store, and the ledger's own records are written by the
ledger without the host.

| Event type | Why |
|---|---|
| `fact.published`, `fact.invalidated` | today's rule |
| `cli.command` | W1 writes it offline (action `owner.unlock`, payload `OwnerUnlock`) |
| `ledger.recovered` | W2 writes it offline |
| `ledger.dump_imported`, `ledger.gap_recorded`, `ledger.health_observed` | the ledger's own records (§9.4, §10, §14) |
| `runtime.fact_snapshot` | sealed by the ledger itself (probe T2) |
| `runtime.failed` | the sealed dump's last record is the ledger's own (§9.2); the type is refused as a whole |
| `artifact.eviction_intent`, `artifact.eviction_outcome` | retention and material eviction run inside the ledger (`global.rs:2030-2084`) |

- AO-0 checked the three resource repositories at their `main` heads `d41137c0`, `6dd529a1` and
  `dfb3729` (fetched 2026-10-12): every `invalidate_on` declared there names only
  `input.committed` and `input.failed`. No resource fact uses a refused type, so the rule refuses
  nothing that exists. The Runtime's own declarations use `runtime.takeover` only.
- The validator change lands with SH-1 (the v0.12.0 shape slice); the rejection is the existing
  request error `invalid_fact_record`.

## 4 Silent gates (G1–G16)

Each gate is one `capture` call at a choke point (LEDG §4.1). *Decision* records are today's
durable class (FULL sync, may be tagged); *observation* records are today's observational class
(NORMAL sync, never tagged). A new event type is a decision record unless listed otherwise.

| Gate | Choke point (LEDG §4.1 lines at 010435c3; 706ead94 lines in Appendix A.6) | Copies | Category | Slice |
|---|---|---|---|---|
| G1 | fan-out point `emit` (`host/facts.rs:511-557`) | every event it emits and the `fact.invalidated` it derives, the recovery ladder's records included (`append_event_raw`) | by type | AO-3 |
| G2 | instance store entry `InstanceFactStore::apply` (`host/facts.rs:196-374`, `:86-167`) | `fact.published` / `fact.invalidated` with (lineage, revision), publisher, scope; add/subtract changes carry the value after the change | decision | AO-3 |
| G3 | workbench entry (`runtime_facts.rs:586-646`, `:799-853`, `:237-345`; install transition `installation.rs:245-344`) | `runtime.fact_recorded` / `runtime.fact_invalidated` with revision, `host.install_transition` included (its receipt waits for the commit) | decision; instance-scoped self-check records observation | AO-3, AO-4 |
| G4 | scheduling queue and leases (`lease.rs` grant/transfer/release/expiry/renewal; queue admit/deny; hand-off) | `scheduler.queued/admitted/denied`, hand-off, `lease.transition_intent` and results | decision | AO-4 |
| G5 | policy decisions (`PolicyHost` evaluation, refusals, clock-slot fates; dispatch intent and result `policy_dispatch.rs:1416`, `:1471`; value records §11.3) | non-intent `policy.*`, `policy.dispatch_intent` and result, `policy.control_state_recorded`, `policy.activity_state_recorded`, `policy.fact_refresh_transition` | decision | AO-4 |
| G6 | input and application (`input.rs:182-195` inside the gate; intents `:172`/`:197`; application `:500`/`:510`) | `input.intent` → `input.committed/failed` and their invalidations; `application.intent` → result | decision, never waited on | AO-4 |
| G7 | task trace (kernel `record(trace)` via host `contained_task.rs:919`, `:3254`, `:1891`; terminal `:7069`) | `task.*` semantics, recognition, evidence; `task.terminal_intent` and terminal | decision: the terminal family, `task.selection_evaluated`, `task.requested`, `task.started`; observation: the rest (Appendix A.8, M10) | AO-4 |
| G8 | capture and recognition pipeline (`observation.rs`, `online_observation.rs`, `monitor_control.rs`, `lab_operation.rs`, `saved_artifact_ocr.rs`) | `capture.*`, `recognition.*`, `monitor.*` | observation | AO-4 |
| G9 | material publication (`ArtifactEventSink`, host `contained_task.rs:3946-3964`; `frame_retention.rs:156`) | `artifact.created/verified/pin_recorded`, `artifact.pin_released` (`release_lab_pin`) | `artifact.created`, `artifact.verified`, `artifact.pin_released` decision (business reads them, X6); others observation | AO-4 |
| G10 | backend open and self-check (`backend_open.rs:8-29`; `lifecycle.rs:31`, `:48`) | `runtime.lifecycle_observed` (backend, self-check); `provider.startup_observed` (`provider_startup.rs:48`) | observation | AO-4 |
| G11 | performance output (`performance.rs`, `performance/capacity.rs:698`; the host `pace_checkpoint` implementation, PERF P-h) | `perf.*` | observation, except `perf.dispatch_share_changed` and `perf.run_hold` (decision) | AO-4 |
| G12 | agent control (`agent_control.rs:144-336`) | `agent.*`; `agent.wake_obligation_recorded` | decision | AO-4 |
| G13 | emulator start/stop (`emulator_instance.rs:105-128`) | intent and result | decision | AO-4 |
| G14 | IPC write-request boundary (`requests.rs:11-120`, `CommandValidation`) | `command.received/validated/rejected` of write requests and of the reads that write records (observe, capture sequence, page observe, planning projections; Workflow #389 §5.2), with `channel` and optional `via`; pure queries are not copied | decision | AO-4 (fields from U-2) |
| G15 | joint capture: approvals (`governance.rs:97`), planning signal and quota (`planning.rs:82`), release staging/activation/rollback and state migration (`state_control.rs:46`, `:113`, `:140`, `:168`, `:324`, `:370`), catalog activation and rollback (`policy_catalog.rs:232`, `:263`, `:327`; `catalog_transaction.rs:268`) | record + state-document work, committed in one transaction by the flush thread; install receipts wait for that commit, approvals and planning do not | decision | AO-4 |
| G16 | process lifecycle (start `host.rs:1543` → `lifecycle.rs:619`, `host.rs:1550` → `lifecycle.rs:672`, start steps §12; shutdown request `host.rs:3029-3048`; formal close `host.rs:3752`; fatal `lifecycle.rs:113-126`; close-time device diagnostic summary `device_diagnostic.rs:205`) | `runtime.started/takeover`, `runtime.instance_bound`, lifecycle phases `tagged_state_loaded` and `tagged_state_enabled`, close, and every `runtime.failed` through its one choke point `append_lifecycle_failure_with_severity` (process stages class A, operation stages class B) | decision; one flush barrier before business opens; drained before close and before a fatal exit | AO-3 |

**Rules for every gate** (LEDG §4.2):

1. After the effect. A store has applied, an input has completed, a file is written; only then is
   the result copied. An intent appears only as an intent type.
2. Observe only. A gate changes no data, returns no error to business and never waits.
3. Hot-path cost: one capture number (under the queue lock), one enqueue, one bounded copy or
   shared reference, two integer comparisons for the watermark, and `observe_trigger` when
   needed. No draft assembly, sanitizing, serializing, hashing or I/O.
4. Never full: past tier 2 the spill thread moves the oldest records to the dump file (§7); the
   capture still returns at once and nothing is dropped.
5. A record that fails assembly or sanitizing on the flush thread is `ledger_record_rejected`
   with a single-record gap (§10); business is not affected.
6. Business acceptance rules stay with the producer (store acceptance, IPC validation); a refused
   change returns to its caller as today and produces no record.

Intent and result stay two records (rule 8: an intent is not a fact) but the effect no longer
waits for the intent to commit (rule 9). Outside readers show an intent without a result as
"in progress or unknown", never "done" (MCP `ac_get_run`, the UI run view; AO-9).

## 5 Call-site classes (AO-0 inventory; AO-3 to AO-4 move them)

Every non-test ledger write in `runtime-host` and `actingd` at `main` 706ead94 belongs to one
class (LEDG §4.1b). Appendix A lists each site.

| Class | What | After AO-4 |
|---|---|---|
| A business event | has a business consumer: a store, the queue, the policy, a notification or an invalidation | the business sends to its consumers as before; a gate at the fan-out point or the entry copies it (G1–G6, G12–G16) |
| B ledger-only record | no business consumer; only views, forensics and frame retention read it (kernel trace, capture and recognition, `artifact.*`, `perf.*`, `monitor.*`, backend and self-check observations, read-only command audit) | taken by the capture interface at its existing choke point (G7–G11); the host no longer assembles a draft. Records of the backend-open, self-check and capture families also feed one in-memory observer, the recovery ladder's direction-1 counter (#387 D1c), placed beside the performance pipeline observation; it reads no ledger |
| W exception table | a record no probe can see: offline subcommands only | the offline process opens its own writer and appends directly (§6) |

## 6 The direct-write exception table (rule 14)

The only condition for a direct write is that no probe can see the record: when it is produced no
capture pipeline is running. The ledger starts first and closes last (§12), so start, takeover,
close, fatal and install in a running actingd are all copied by gates.

| # | Call site | Why no probe sees it | Record |
|---|---|---|---|
| W1 | `actingd unlock-owner` (`apps/actingd/src/owner_unlock.rs`) → `crates/runtime-host/src/owner_unlock.rs` (`ledger.append`; writer opened through `LedgerMaintenance::acquire` and `open_writer`) | offline subcommand: runs only after the previous owner has exited; no actingd, no capture pipeline; the subcommand is the only writer | `cli.command`, action `owner.unlock`, payload `OwnerUnlock` (LEDG calls it `client.owner_unlock`; [actingd unlock-owner](actingd-unlock-owner.md)) |
| W2 | `actingd ledger-maintenance import` (`apps/actingd/src/ledger_maintenance.rs`) → `crates/runtime-host/src/ledger_maintenance.rs` → `LedgerMaintenance::import` (`crates/ledger/src/global/migration.rs`); `dry-run` writes nothing | offline subcommand: takes the owner first, so actingd cannot run; no capture pipeline | `ledger.recovered` (`reason` `storage_cutover`, with the migration record) |

- The other operations of the same subcommand do not write: `verify` and `backup` read;
  `restore` copies a backup and appends nothing.
- An offline direct write appends synchronously as today; on failure the subcommand exits with a
  registered code (Fail Loud). An offline process has no dump and does not follow §9.

**Not in the table** (LEDG §6.1):

| Candidate | Why not |
|---|---|
| the last fatal record while the capture pipeline itself is failing | capture never refuses; the fatal record is queued and sealed into the dump by the ledger (§9.2); when even the dump cannot be written, the last word goes to stderr and the fallback log through the start-refusal writer, and the next start writes a tail gap |
| acsetup install steps while actingd is not running (cold path) | acsetup does not write the ledger (`component-interfaces.md`: only actingd writes `ledger`); the steps run later inside a started actingd and go through G15. Checked by AO-0 on the UI repository's `main` `8060ba5`: `crates/acui-setup` does not depend on `actingcommand-ledger` and calls no ledger write; only the console's `acui-source` reads the ledger |
| install inside a running actingd | G3 (`host.install_transition`) and G15 (release, state migration, catalog), receipts after the commit |
| refusals before the ledger opens (invalid configuration, `perf_config_invalid`, …) | stderr and exit 1 as today; there is no ledger yet |
| the ledger's own records | produced by the ledger module, not requested by the Runtime body: dump import, tail and single-record gaps, the sealed dump's `runtime.failed`, probe findings, the self-sealed snapshot, retention and material eviction, and the records the ledger drafts when start triggers it (prior-epoch imports and scope closes, startup eviction recovery; Appendix A.8, M3) |

## 7 Capture queue and watermarks (AO-2)

- **One queue in capture order** with two segments: memory, and the spilled dump segment. Always:
  committed < batch in flight < dump segment < memory segment, all in capture order. The
  category only chooses the sync level and whether the record may be tagged.
- **Budget**: `buffer_bytes` and `buffer_records`, whichever is reached first (§15).
- **Three tiers**, the frame store's shape (`frame_store.rs` defaults 0.60 / 0.75 / 0.90,
  hysteresis 0.10):

| Tier | Ledger capture queue |
|---|---|
| 1 (default 60 %) | `ledger_capture_buffer_busy` (warning, reported per segment); the flush thread uses `flush_batch_records_busy`; records are never merged or deduplicated |
| 2 (75 %) | the spill thread appends the oldest memory records (the batch in flight excepted) to the dump file in capture order until the tier-2 release line; `ledger_capture_spilled` (warning, per segment, with count and bytes) |
| 3 (90 %) | `ledger_capture_buffer_overloaded` (warning, needs a person): spilling cannot keep up; the spill thread now spills everything but the batch in flight; business is not paused |
| spill write fails, memory reaches 100 %, or the dump reaches `dump_max_bytes` | §9.2: dump and fatal; a dump that cannot be written either is `ledger_dump_failed` |
| hysteresis | release line = tier × (1 − `hysteresis_ratio`); the segment's end is reported when the release line is crossed |
| configuration check | tier1 < tier2 < tier3; each release line below its activation line; tier3 − tier2 ≥ `spill_batch_bytes`; otherwise refused at start with a registered code |

- The spill thread and the flush thread are separate: a flush stuck inside SQLite does not stop
  spilling.
- The dump file is the one of §9.2. In normal running it is the spill destination: the flush
  thread reads the dump segment first, then memory, and deletes the file once all of it is
  committed. So "the dump is the oldest unflushed part" always holds and a crash leaves it on disk
  for the next start (§9.4).

- **As built by AO-2** (register §1.38):
  - The tier is the higher of the records tier and the bytes tier. A record's bytes are its
    sanitized draft's serialized size. The spill thread measures them, and it also assembles and
    sanitizes captured items ahead of the flush; the flush prepares a front record itself when
    the spill thread has not reached it. A record not yet measured counts as one record and no
    bytes.
  - The spill thread does not spill past a joint record (G15): its state work cannot be dumped,
    so it and every record after it stay in memory.
  - At the hard limit (100 % of either budget), a spill that fails, or a spill that would take
    the dump file past `dump_max_bytes`, the capture still returns at once, and the spill thread
    seals the writer and writes §9.2's dump without waiting for the flush. The dump holds the
    spilled segment, the batch in flight and memory, then the fatal record, in capture order.
  - A segment's start and end (`ledger_capture_buffer_busy`, `ledger_capture_spilled`,
    `ledger_capture_buffer_overloaded`) are `ledger.health_observed` records that enter the queue
    at the edge, `segment_edge` `started` or `ended`. Each carries the records and bytes (`count`,
    `buffer_bytes`: in memory for busy and overloaded, spilled so far for spilled), and an end
    carries the segment's `elapsed_ms`. The spilled segment ends when the dump file is deleted.
    Once the close has begun (§8.2), edges are no longer reported, so the close record stays the
    epoch's last.
  - Only the running Runtime's writer (opened with a failure plan, §9.1) spills and reports
    tiers. An offline writer has no dump place, keeps its records in memory and reports nothing.
  - The queue's policy (`LedgerCapturePolicy`) comes from §15's keys through the host's failure
    plan. A zero budget, batch or ring size, an empty or blank workbench pattern, or ratios that
    fail the check for either budget (bytes with `spill_batch_bytes` as the gap, records with 1)
    are refused when the writer opens: `invalid_ledger_config` at `configure_ledger_capture`.

### 7.1 Shared watermark module (AO-2, after #664)

The pure arithmetic of the frame store's tiers moves into a module of `crates/artifact-store`
that is outside the `capture` feature, so the ledger (which depends on artifact-store with
`default-features = false`) can use it. The frame store calls it; its behaviour and tests stay
unchanged.

```rust
pub struct WatermarkRatios { pub tier1: f64, pub tier2: f64, pub tier3: f64, pub hysteresis: f64 }
impl WatermarkRatios {
    /// tier1 < tier2 < tier3 <= 1; each release line below its activation line;
    /// (tier3 - tier2) * budget >= min_gap (the frame store's flush workspace, the ledger's spill batch).
    pub fn validate(&self, budget: u64, min_gap: u64) -> Result<(), WatermarkConfigError>;
}
pub enum WatermarkTier { Below, Tier1, Tier2, Tier3 }
/// The tier for `used` of `budget`, given the previous tier: activation at a tier's ratio,
/// release only below tier × (1 − hysteresis).
pub fn watermark_tier(ratios: &WatermarkRatios, used: u64, budget: u64, previous: WatermarkTier) -> WatermarkTier;
```

As built by AO-2: `WatermarkRatios::lines(budget)` gives the activation and release lines as
`WatermarkLines`, with `tier(used, previous)` and `validate(min_gap)`; `watermark_amount(amount,
ratio)` is the rounding both callers share. The frame store's `MemoryBudget` uses them with its
messages unchanged.

## 8 Flush (AO-2)

### 8.1 Order and sync

- One flush thread (today's writer thread). It reads the batch in flight, then the dump segment,
  then the memory segment; assembles and sanitizes; commits one batch.
- The capture number is taken and the record enqueued under one queue lock: with several
  capturing threads, capture order = enqueue order, without holes. Ledger sequences are given in
  capture order, so ledger order = capture order, and for captures inside the gate ledger order =
  business order.
- Every stored record carries `capture_seq`, its source and source (lineage, revision) when it has
  one (§8.3), and its capture time; `timestamp` is the capture time.
- A batch that holds a decision record commits with FULL sync, otherwise NORMAL.
- **A tagged record commits on arrival** (rule 13): the batch ends at it, FULL sync, without
  waiting for the batch to fill.
- **The index table is maintained in the same SQLite transaction** as the batch (§11.4).
- **As built by AO-2**:
  - A batch is taken from the front of the queue, the dump segment first, up to
    `flush_batch_records`, or `flush_batch_records_busy` from tier 1 on. A joint record (G15)
    commits alone. A tagged record ends its batch. The batch syncs FULL when it holds a decision
    or tagged record, otherwise NORMAL.
  - The writer is the flush thread. It drains the queue before each command it serves and at
    its end. Direct appends (`append`, `append_deferred`, `append_transaction`) pass through the
    queue too, with the draft's own time as their capture time. Their callers still wait for the
    commit until AO-3.
  - The capture envelope (register L4) is written on every record that passes the queue. The
    capture number counts from 1 for each writer that opens. The records the writer drafts
    itself inside a command are written without an envelope until AO-3 and AO-4 route them
    through the gates: retention and eviction, the Lab pin, prior-epoch closes, the settlement
    continuation, recovery records, the import's own `ledger.dump_imported` and gap records,
    and the tail gap. An imported record keeps the capture of its dump line.
  - A refused capture is written as its `record_rejected` gap in its place, and the batch's
    other records are written. A refused direct append answers its caller, and no gap is
    written.

### 8.2 Shutdown

Business stops; the close record enters the queue through G16 and is the epoch's last record; the
flush thread drains the dump segment and the memory segment; the ledger closes last. A fatal exit
for any other reason does the same with `runtime.failed`. If the drain cannot be written, §9.2.

As built by AO-2: `GlobalLedger::shutdown` marks the queue as closing, sends the shutdown record,
lets the writer drain both segments, stops the spill thread and closes. An append after the
close is refused at once (`writer_unavailable`). A capture after the close still returns its
ticket but is never written, and its barrier answers `writer_unavailable`; G16 (AO-3) stops
business before the close. Until G16, the close record is the shutdown request
(`runtime.lifecycle_observed`, `shutdown_requested`).

### 8.3 Revisioned sources and continuity

- Registered sources: the instance store (`instance_facts`), the workbench (`runtime_facts`), the
  decision memory (`decision_memory`) and the D2 transition recorder
  (`fact_refresh_transitions`, Workflow #384 S5).
- Per (source, lineage) the revisions in the ledger are contiguous. The first record of a new
  lineage follows its base record (`tagged_state_loaded`, §12 step 7). A missing stretch may be
  covered only by single-record gaps (§10); otherwise the flush thread reports
  `ledger_store_revision_skipped` (error, needs a person): direct evidence that a business write
  bypassed the gate. It compares two integers.
- The end of the previous lineage missing after a crash is not a skip: the lineage never
  continues, and the tail gap explains it.
- The D2 transition recorder: lineage = the owner epoch, revision = the transition number, from 1
  in each epoch, +1 per transition, never reset per cycle; the cycle number travels in the payload
  and takes no part in the check. It is not loaded and not tagged; its first transition can only
  follow the base record of §12 step 7.
- As built by AO-2: the flush thread keeps the last revision of each (source, lineage) it has
  committed in this process. A record more than one past it is written, and is followed by
  `ledger_store_revision_skipped` with `ledger_source`, `source_lineage`, `expected_revision` and
  `source_revision`. A refused record's gap covers its revision. AO-2 writes whatever revision an
  item carries; the stores set it from AO-3.

### 8.4 Flush barrier

Waiting on the barrier returns only when the record is committed; the only other ending is the
process stopping fatally after retries are exhausted (§9.2). A stalled attempt is bounded by
`flush_attempt_stall_ms` (§9.1). The three users are listed in §2.

As built by AO-2: §2.1, "As built by AO-2". The barrier asks the writer for the committed
record's sequence by event id. It never writes, and it does not wait for records captured after
its own.

## 9 Write failure: retry, dump, fatal (AO-1)

### 9.1 Rules

- Three kinds of failure:
  - **write failure** (I/O, busy, timeout, disk full) → retry;
  - **record refused** (sanitizing or validation) → no retry; `ledger_record_rejected` and a
    single-record gap;
  - **integrity or format** (hash chain, `ledger_format_unsupported`, checksum) → fatal at once,
    no retry and no further ledger write; unflushed records still go into the dump, and the next
    start imports them after a person repaired the ledger.
- **Retry at once**, same batch, same event ids; `write_attempts` attempts with
  `write_retry_backoff_ms` before each.
- **Read back before every retry** (`ledger-store.md`: a failed append is not proof of absence):
  read the database head and the batch's event ids; ids already present count as written and
  move the memory head and the index mirror; the rest are written again with their original ids.
- **The store stays usable after a failed write**: today an I/O error is terminal and the writer
  thread exits. AO-1 rolls the memory head and indexes back to the last confirmed commit and
  reopens the connection when needed before retrying.
- **A stalled attempt**: a stuck fsync never fails. The probe thread (T8) watches the start time
  of the current attempt; beyond `flush_attempt_stall_ms` it treats the attempt as exhausted and
  runs §9.2 itself; the batch in flight is still in memory and goes into the dump (if it was
  committed after all, the import skips it by event id).
- **Waits for a read** keep the writer reply limit and end in the registered error
  `writer_response_timeout` (no longer fatal).
- Reports: `ledger_write_retried` (warning, merged per segment, `attempts`);
  `ledger_write_failed` (fatal, `attempts`, `io_kind` of the last failure; `timeout_ms` when the
  attempt stalled).
- **As built by AO-1** (before AO-2's capture queue):
  - The rules hold for the running Runtime's writer, opened with a *failure plan*: the write
    policy of §15, the two dump places and the sink that reports and stops
    (`LedgerMaintenance::open_writer_timed` with a plan). An offline writer (§6) has no plan:
    one attempt, and its first failure ends it as before.
  - A batch is one record. The unflushed records are the record in flight and the plain appends
    still queued for the writer. A joint record (`append_transaction`, G15's work) is never
    dumped: its state work rolls back with it, so neither took effect, and its caller gets
    `ledger_write_failed`.
  - Write failures are SQLite's I/O, busy, locked, read-only, full-disk, permission,
    out-of-memory, interrupted, cannot-open and lock-protocol results. Any other failure on the
    write path (a constraint, a head that moved, a connection the database refuses) is an
    integrity failure. The database has one shared connection and no reopen: a connection
    refused after a failed sync restore fails every retry and ends in §9.2.
  - `attempts` of `ledger_write_retried` counts the failed attempts of a write that committed in
    the end; it is reported once per such write through the plan's sink (actingd's
    `WARN actingd:` and `OUTCOME actingd:` lines). `attempts` of `ledger_write_failed` counts the
    attempts made. AO-8 brings the counts to status.

### 9.2 Exhausted: dump, then stop fatally (rule 11)

1. **Seal**: take the queue lock and mark "dumping"; capture keeps accepting and the seal takes
   those records too.
2. **Fatal record**: the ledger (flush or probe thread) enqueues `runtime.failed` with outcome
   `ledger_write_failed`, `caused_by` the last failure. It is the ledger's own record.
3. **Write the dump**: append the batch in flight and the memory segment in capture order (the
   spilled segment is already there), write the trailer line, `sync_all`.
4. **Stop**: write the OUTCOME line through actingd's start-refusal writer (stderr and the
   fallback log) and exit fatally. The watchdog does not restart after a fatal; a person repairs
   the disk or file and starts again; the next start imports (§9.4).

As built by AO-1: the writer seals itself when a write gives up; the probe seals it when an
attempt stalls (T8). Every queued command is answered with `ledger_write_failed`. The plan's
sink receives `ledger_write_failed` with the dump's `path`, or `ledger_dump_failed`; actingd's
sink writes `FATAL actingd: <code>` and the `OUTCOME actingd:` line in one write to stderr and
exits with code 2 (A6a brings the start-refusal writer's fallback log and the watchdog's
reading of the exit). A dump that would pass `dump_max_bytes` keeps the records that fit and no
trailer, so its import writes a `dump_incomplete` gap for the rest.

The v1.1 degraded mode, `ledger_write_unconfirmed` and the 10-minute halt do not exist.

### 9.3 Dump file

- **Location**: `ledger-dump.jsonl` in the state root (beside `runtime-state.sqlite`). When the
  state root cannot be written (most realistically a full disk), the system drive's
  `%LOCALAPPDATA%\ActingCommand\ledger-dumps\ledger-dump-<h>.jsonl`, where `<h>` is the first 16
  lowercase hex digits of the SHA-256 of the state root's canonical path (UTF-8). A start checks
  both places.
- **Format**: JSON Lines, UTF-8, one JSON object per line, `\n` after every line.
  - **Header** (first line): `{"dump_version": 1, "state_root": <path>, "owner_epoch": <n>,
    "runtime_commit": <40 hex>, "started_at_unix_ms": <n>}`.
  - **Record line**: `{"record": <object>, "sha256": <64 hex>}` where `record` is
    `{"capture_seq", "event_id", "category" ("decision"|"observation"), "captured_at_unix_ms",
    "draft"}` (`draft` = the sanitized record draft, joint work and source revision included)
    and `sha256` is the SHA-256 of the exact bytes of the `record` member as written (the reader
    takes them as a raw value; nothing is re-serialized).
  - **Trailer** (only written by §9.2 step 3): `{"trailer": {"count", "first_capture_seq",
    "last_capture_seq", "lines_sha256", "reason_code"}}`; `lines_sha256` is the SHA-256 of all
    record lines' bytes (newline included) in file order; `reason_code` is the code that caused
    the seal.
  - Spilling appends record lines only; so "has a trailer" means "a complete sealed dump".
  - A reader tolerates unknown members. A `dump_version` newer than the reader is
    `ledger_dump_unsupported`.
- The dump holds sanitized drafts only; it is private to the state root and never exported. Its
  existence, size and import result are reported through actingctl and IPC, never its content.
- As built by AO-1 (register §1.26 AO1): the header's `owner_epoch` is the owner epoch's id
  (`epoch_…`); `runtime_commit` is present when the build knows its commit. A record's `draft`
  holds the sanitized draft's members (`schema_version`, `event_id`, `timestamp_unix_ms`,
  `event_type`, `severity`, `sensitivity`, `origin`, `links`, `payload_schema`, `payload`, and
  `artifacts` as projected references with their object keys); `captured_at_unix_ms` is the
  draft's time until AO-2 captures; `capture_seq` counts from 1 in each dump. Every `sha256` and
  `lines_sha256` is 64 lowercase hex digits. A place that already holds a dump file is not
  written to; the next place is tried.

### 9.4 Next start: import (AO-1)

After the ledger is opened and verified, before any new record (§12 step 2):

1. look in the state root and the fallback folder; no dump → skip;
2. check: `dump_version` newer than this build → refuse to start, `ledger_dump_unsupported`
   (fatal), file untouched; verify every line's hash;
3. sort by `capture_seq` (spilled and sealed segments may interleave); skip records whose event id
   is already in the ledger (the existing event-id index, no full scan);
4. append in batches with the rules of §9.1 (retry, read back), maintaining the index table in the
   same transaction;
5. write `ledger.dump_imported` (outcome `ledger_dump_imported`, info; the ledger's own record):
   imported count, skipped duplicates, whether the trailer was present, the dump's `lines_sha256`;
6. once that record is committed, delete the dump file.

- A crash during import leaves the dump; the next import skips what was imported (by event id).
- **Damaged dump**: no trailer and a truncated last line → import the verifiable part, write a
  gap (`ledger_dump_incomplete`, warning), delete the dump. A line in the middle whose hash does
  not match → import the verified prefix, write a gap (`ledger_dump_invalid`, error, needs a
  person), rename the file to `*.invalid-<unix_ms>.jsonl` and keep it unchanged.
- The ledger still cannot be written during import → refuse to start (`ledger_write_failed`),
  dump untouched.
- An epoch that ended in a complete sealed dump (trailer, last record `runtime.failed`) is
  complete: no tail gap is written for it.
- As built by AO-1: the import runs inside the writer's open, after the ledger is verified and
  before the stale-owner record. Both places are read first, so a newer version refuses the
  start before anything is written; then each dump is imported, the older epoch first. A dump
  without a trailer is incomplete whether or not its last line is cut; a trailer whose count or
  `lines_sha256` does not match the verified lines makes the dump invalid. Imported records are
  not held back by a retention recovery still pending: they precede every record of this start.

### 9.5 Fatal paths removed or changed (AO-1 unless named)

| Today | After |
|---|---|
| writer reply 10 s → fatal `writer_response_timeout` (`global.rs:68`, `:2481-2491`) | a reader's wait ends in error; a flush attempt is watched by T8 (`flush_attempt_stall_ms`) |
| I/O error terminal, writer thread exits (`global.rs:541-548`, `:2111-2119`) | roll back, store stays usable, read back, retry; exhausted → §9.2 |
| ingress full → fatal `ingress_full` (`global.rs:2472-2474`) | three tiers and spilling; only the hard limit or a failing spill → §9.2 |
| one failed append latches `lifecycle_append_failed`, every later request refused (`host/facts.rs:592-594`, `:621-624`, `:633-636`, `:650-652`; `host.rs:3029-3038`) | latch removed; business never sees a ledger error; the process stops by §9.2 |
| a ledger error poisons the host → `fatal.mark` (`requests.rs:91-96`; `host/facts.rs:368-372`, `:687-691`) | business never sees ledger errors (gates return none) |
| workbench append failure or memory refusal → `runtime_fact_store_desync` (`runtime_facts.rs:602-606`, `:640-642`) | the store accepts first; "ledger has it, store refuses" cannot happen (AO-4) |
| policy monitor subscription timeout 5 s → FATAL (`main.rs:46-48`, `:652-667`) | the driver no longer subscribes to the ledger (AO-7) |
| deferred appends not confirmed at close → fatal (`global.rs:1825-1855`) | the close drain retries by §9.1; exhausted → dump then fatal; nothing is dropped |
| frame retention: ledger error → fatal (`frame_retention.rs:83`) | read errors return an error; writes go through G9 |
| background threads mark fatal on a ledger error (`performance.rs:428-443`, `host.rs:3933-3951`, `monitor_control.rs:719-738`) | ledger write errors no longer reach them (AO-4) |

Still fatal: integrity or format failure; `ledger_write_failed`; `ledger_dump_failed`;
`ledger_dump_unsupported`; `ledger_tag_index_mismatch` (§11.5).

AO-1 built the writer reply row (a read's wait ends in `writer_response_timeout`, an error), the
I/O error row (for the running Runtime's writer), the latch row (`lifecycle_append_failed` is
gone) and the read half of the frame retention row (a read the writer did not answer in time is
an error of the cleaner's round, recorded, and the next round reads again). The `ingress_full`
and close-drain rows need AO-2's capture queue, the poison row AO-3's gates, the workbench and
background-thread rows AO-4; the policy monitor row was AO-7's.

## 10 Gap records (AO-1, AO-2)

Event type `ledger.gap_recorded` (family `ledger`; the ledger's own record), payload
`{gap_kind, gap_start_unix_ms, gap_end_unix_ms, record_count (absent = unknown), source?,
event_id?, outcome}`:

| `gap_kind` | When | Time range | Count | Code |
|---|---|---|---|---|
| `dump_incomplete` | import: no trailer, last line truncated | last verified record's capture time → the dump file's last-modified time | unknown | `ledger_dump_incomplete` (warning) |
| `dump_invalid` | import: a middle line's hash differs | same, from the last verified record | unknown | `ledger_dump_invalid` (error, needs a person) |
| `tail_unconfirmed` | start: the previous epoch has neither a close record nor a complete dump | last committed (or imported) record's time → this start | unknown, always | `ledger_tail_unconfirmed` (warning) |
| `record_rejected` | flush: one record failed assembly or sanitizing | that record's capture time | 1; `event_id` and, for a revisioned source, `source` = (source, lineage, revision) | `ledger_record_rejected` (error, needs a person) |

- Effects done before a crash whose records never committed (rule 9) and an observational tail
  lost to a power cut fall inside the tail gap's range.
- Gap records join the Errors view (severity ≥ warning) and the Health view (§14.3). `status` and
  MCP show the gap count of the last `status_gap_window_ms` and the latest gap.
- **Frame retention** (#375, AO-9, needs an update of m375): frames whose error window overlaps a
  gap's time range count as "evidence incomplete"; classes 4 and 5 do not delete them; they are
  judged again at the next retention period.
- As built by AO-2:
  - `record_rejected`: the gap has an event id of its own, at the refused record's capture time
    and with its capture number. Its `outcome` is `ledger_record_rejected`, caused by the
    refusal's code, with `event_id` and, for a revisioned source, `ledger_source`,
    `source_lineage` and `source_revision`.
  - `tail_unconfirmed`: written once by the running Runtime's writer when it opens, after the
    dump import, when the previous owner was not closed cleanly and no imported dump ended
    sealed. The range runs from the last committed (or imported) record's time to the open.

## 11 Tags and the index table (AO-2 maintains, AO-5 rules and seeding, AO-6 loads)

### 11.1 The index table `ledger_index`

The two state files of model v1.1 do not exist. The tag lives in an index table inside the ledger
database (`runtime-state.sqlite`), not in any record's serialization:

```sql
CREATE TABLE ledger_index (
    role           TEXT    NOT NULL CHECK (role IN ('state', 'open', 'request', 'lifecycle')),
    key            TEXT    NOT NULL,
    sequence       INTEGER NOT NULL,   -- the record's ledger sequence
    event_id       TEXT    NOT NULL,
    value_sha256   TEXT    NOT NULL,   -- SHA-256 of the record's value as stored
    source_lineage TEXT,               -- for records of a revisioned source
    source_revision INTEGER,
    PRIMARY KEY (role, key)
) WITHOUT ROWID;
```

| `role` | `key` | Row life |
|---|---|---|
| `state` | the tagged key: `<source>/<scope>/<fact key>` for the stores, `input_boundary/<instance>/committed` and `/failed` for the input boundaries, `decision_memory/<instance>/<task>` for a dispatch pair, `activity_memory/<instance>/<activity profile>` for an activity state, `wake/<obligation id>` for a wake obligation | overwritten by the key's latest record; "tagged" = has a `state` row |
| `open` | an intent's event id | added when the intent commits, deleted when its result commits (same transaction); read by X1 |
| `request` | a write request id | ring of the latest `request_index_records` rows; read by X3 |
| `lifecycle` | `last_start`, `last_close`, `last_fatal` | overwritten; read by X2 |

- Only `state` rows load into business memory; the other roles serve the exception reads.
- Rows are bounded: instance store ≤ 256 active facts plus tombstones; workbench ≤ 16 384 and only
  whitelisted keys; decision memory one per dispatch pair and one per (instance, activity
  profile); requests bounded by configuration.
- The index is internal; outside readers read records and views. A mirror of the table lives in
  the ledger's memory (§14.1).
- **As built by AO-2** (register §1.38 OB1):
  - The table is `STRICT, WITHOUT ROWID`. `sequence` and `source_revision` use the
    ledger's ordered integer encoding. `value_sha256` is the record's `record_sha256`, the hash of
    its canonical record. The source columns come from the record's capture.
  - `state` keys: `instance_facts/<scope>/<fact key>`, where the scope is `instance:<id>`,
    `server:<id>` or `game:<id>`, one row per record of a `fact.published` and the key of a
    `fact.invalidated`; `runtime_facts/runtime/<key>` and
    `runtime_facts/instance:<id>/<key>` for workbench keys that match a
    `tag.workbench_patterns` pattern (`*` = any text); `input_boundary/<instance>/committed` and
    `/failed`; `decision_memory/<instance>/<task>`; `activity_memory/<instance>/<profile>`;
    `wake/<wake id>`.
  - `open`: the intents `policy.dispatch_intent`, `lease.transition_intent`,
    `task.terminal_intent`, `input.intent` and `application.intent`. A result (dispatch admitted
    or rejected, lease granted, transferred, renewed, released, expired or failed, task
    completed, failed, cancelled or commit failed, input committed or failed, application
    completed or failed) deletes the `open` rows of the intents that share its action id.
  - `request`: the latest `command.received`, `command.validated` or `command.rejected` record
    of each request id. Rows past the newest `request_index_records` are deleted in the same
    transaction.
  - `lifecycle`: `last_start` for `runtime.started` and `runtime.takeover`; `last_close` for the
    shutdown request (the close record until G16); `last_fatal` for a fatal `runtime.failed`.
  - Every batch writes its rows in its own transaction, including imports, gaps and the
    writer's own records. A batch the store refuses rolls back with its rows, and a read-back
    retry (§9.1) finds both or neither.
  - There is no migration. A database created before AO-2 has no table, so its first batch
    fails (§9.2). Under the 10-11 ruling, the test stage keeps no reader for older databases.

### 11.2 Which keys are tagged (configuration, LEDG §8.2)

| Class | Default rule | Record type |
|---|---|---|
| instance store | every key: active facts (`session.*`, resources, inventory, `health.perf_baseline.*`, feature locks, pack schedules, instruction lines), the per-instance input boundaries, and any identity table stored there | `fact.published` / `fact.invalidated`; the latest `input.committed` and the latest `input.failed` per instance (two boundaries, `fact_store.rs` `InputInvalidationBoundaries`) |
| workbench | `ledger_observer.tag.workbench_patterns` (§15) | `runtime.fact_recorded` / `runtime.fact_invalidated` only |
| decision memory | one record per dispatch pair (catalog task or the reserved identity of a D2 recognition package R_I); one record per (instance, activity profile) | `policy.control_state_recorded`; `policy.activity_state_recorded` |
| agent wake obligations | one per obligation; sessions are not kept | `agent.wake_obligation_recorded` |

- Never tagged: queue, holders, claims, leases, running runs, the recovery ladder window (one
  queue per instance); the settlement idempotency cache; detection quota and planning signals
  (their home is the state document, G15); the in-memory pack registry `host.pack_registry.*`
  (Workflow #389 §6.5).
- `ledger_observer.tag.reobserve_prefixes` lists workbench keys known to be re-observed or
  replayed in each session; a workbench key matching neither list is not tagged and reports
  `ledger_tag_key_unclassified` (warning) once per prefix.
- Instance store and decision memory are always tagged; there is no switch.

### 11.3 Value records: the latest record is the current value

- Every tagged record carries the key's complete value after the change. `fact.published` and
  `runtime.fact_recorded` already do; an invalidation means "empty". An add or subtract change
  carries the value after the change (it may also carry the delta); amounts come only from live
  recognition, never from a constant in a package (D2 Q-D2).
- The decision memory gets value records (AO-4): every change of a dispatch pair's state emits one
  `policy.control_state_recorded` through G5 with the pair's complete state: daily counts with
  their day key, window counts and the window's runtime (today's `task_runtime`) with the active
  window they count in (the `window_id` of the task's activity profile), failure streak
  (split as the execution record's failure is since Workflow #378 A2-P: `failure_code` and
  `failure_identity` = the failure identity, format v2, Workflow #336 / M-PKG §3.4, and
  `paused_on` rebuilt from it — no package digest is stored), retry round, dispatch clock, and
  `condition_reruns` `{failure_code, count, expires_at_unix_ms}`.
- Every change of an (instance, activity profile)'s activity state emits one
  `policy.activity_state_recorded` through G5 with its complete state: the daily count with its
  day key, the window count and the window's runtime with their `window_id`, and
  `next_eligible_unix_ms` (today's `activity_daily`, `activity_window`, `activity_runtime` and
  `next_activity_eligible`, `policy_control.rs:88-91`). Together the two records hold the whole
  `PolicyControlState`, so a restart after AO-6 keeps runtime budgets and activity intervals
  (register L7, L14).
- Wake obligations get `agent.wake_obligation_recorded` (one `AgentWakeRecord` as of AO-4).
- Derived on load for the instance store: `latest_observed` = the latest record's observation
  time; tombstones = keys whose latest record is an invalidation (≤ key limit; after a tombstone
  rolls out, a resend of the same identity is still refused by `latest_observed` as
  `fact_observation_not_newer`, no longer `fact_source_snapshot_invalidated`); the input
  boundaries = the instance's latest `input.committed` and latest `input.failed` timestamps.
- AO-0 checked the payloads (LEDG §8.3): `fact.published` carries the record with its
  observation time, `fact.invalidated` carries scope, key, snapshot id and time, and the input
  records carry their instance link and timestamp, so the load needs no new member. LEDG §8.2
  names only `input.committed`; today's store keeps a boundary for `input.failed` too
  (`fact_store.rs`), so both are indexed.
- Readings keep their `observed_at` and `expires_at`; after loading they expire as before and go
  through D2.

### 11.4 Write: one transaction, tagged records commit on arrival

- Each batch appends its records and updates their index rows (`state` overwrite, `open` add or
  delete, `request` ring, `lifecycle` overwrite) in one SQLite transaction. A failed transaction
  leaves neither; the retry follows §9.1. The index therefore always matches the committed ledger.
- A tagged record commits on arrival with FULL sync; a crash loses at most one commit.
- A tagged record lost in a crash leaves the key at its previous committed value on the next start;
  the tail gap says so.
- **Exception for `condition_reruns`** (N2, narrowed): when this start wrote a gap (dump damaged at
  step 2 or tail gap at step 5 of §12), step 7 loads the `condition_reruns` of every dispatch pair
  whose task commits on entry (`cost_commit`, Workflow #386 §4.6; the task-to-entry-step table
  compiled from package declarations only, before step 7) as **empty**: the rerun is no longer
  owed. Other tasks keep the previous value. Step 8 writes one value record for each emptied pair.
  Without that table (before SR-10) no pair is emptied. The gap record makes the loss visible; no
  extra code.
- Importing a dump updates the index in the same transactions.

### 11.5 Start loading (AO-6)

1. read `ledger_index` completely (rows = keys, bounded);
2. read each `state` row's record by its `sequence` (point reads, bounded);
3. check each: record key = row key and record hash = `value_sha256`; otherwise
   `ledger_tag_index_mismatch` (fatal: an invariant of persisted state is broken and would recur
   at every start);
4. apply the records to the two stores and the decision memory with today's reducers (only their
   input changes);
5. entries of instances no longer in the configuration are dropped:
   `ledger_tagged_entries_dropped` (info, count);
6. report `ledger_tagged_state_loaded` (info; counts per source, records read, elapsed).

Then both stores open a new lineage and G16 writes the base record (§12 step 7). The read volume
equals index rows plus as many point reads, whatever the ledger's length (test AO6-a). The load
time L that D2 needs (D2 A-7) is the end of step 7.

| Situation | Code | Then |
|---|---|---|
| new state root (the v0.12.0 ledger's first start) | `ledger_tagged_state_loaded` (0 keys) | empty stores; write `tagged_state_enabled` |
| enabled marker present | `ledger_tagged_state_loaded` | load as above; a missing key has no value |
| history without the marker (first start of the build that brings AO-5 + AO-6) | `ledger_tagged_state_seeded` (info) | seed once (§11.6) |
| an index row disagrees with its record | `ledger_tag_index_mismatch` (fatal) | refuse to start |

### 11.6 Seeding once, exception reads

- **Seeding** (AO-5): with history but no `tagged_state_enabled`, step 3 runs today's recovery
  reducers once (read only); step 8 writes every tagged key as a batch of new-lineage value records
  (`ledger_tagged_state_seeded`) and then the enabled marker. Later starts never replay.
  The v0.12.0 ledger starts empty, so the replay is small; the whole-ledger read for seeding
  retires in the next minor version; the reducers stay for §11.5.
- **Exception reads**:

| # | Reads | Why | How |
|---|---|---|---|
| X1 | the previous epoch's committed open intents (dispatch, lease, task, input, application, emulator) | settle effects that may have happened | `open` rows; bounded |
| X2 | hot install: the previous owner drained and closed | safe hand-over | `lifecycle` rows |
| X3 | request-id idempotency across restarts | a resent request must not act twice | `request` rows (ring) |
| X4 | state migration reconciliation | state document and ledger written in one transaction | read by type, small (today's path) |
| X6 | verified materials (planning proposals and strategic reports) | business input | incremental read of one type, after a flush barrier |

  An intent that never committed is not found by X1; it lies in the tail gap (rule 9's accepted
  cost). The same holds for the last request ids and X3.

## 12 Start and shutdown order (AO-6; G16 from AO-3)

1. take the owner; open the ledger; verify it (today); start the capture queue and the flush
   thread at once — every later record goes through `capture`;
2. import the dump (§9.4), `ledger.dump_imported`;
3. read and check the index (§11.5 steps 1–3); without the enabled marker, replay once (§11.6);
4. exception reads X1–X4 (read only);
5. previous epoch incomplete (no close record, no complete dump) → tail gap;
6. through G16: `runtime.started` / `runtime.takeover`, `runtime.instance_bound`;
7. load the stores and the decision memory from step 3, open new lineages; through G16 the base
   record: `runtime.lifecycle_observed` phase `tagged_state_loaded`
   `{stores: [{source, previous_lineage?, previous_revision?, lineage, loaded_count}]}`, one entry
   per store (register L5; previous revision = the highest committed revision of that source in
   the index; both previous members are absent on a new state root);
8. seeded (§11.6): the batch of value records, then `tagged_state_enabled`; new state root: only
   the marker; emptied `condition_reruns` pairs (§11.4): one value record each;
9. X1 recovery results, through `capture`;
10. one flush barrier: the records of steps 2–9 are committed; then business opens.

- Nothing is written before step 6 except the import of step 2, the gap of step 5, actingd's
  provider-assembly observations (`provider.startup_observed`, G10, captured from step 1;
  Appendix A.8 M2) and the ledger's own records that start triggers (the prior-epoch imports and
  scope closes and the startup eviction recovery; A.8 M3).
  Reconciliations that today append before `runtime.started` (`reconcile_policy_dispatches`,
  `host.rs:1475`) move to step 9 (AO-6), so a new lineage's first change follows its base record.
- Every G16 record goes through `capture`, so facts with `invalidate_on: [runtime.takeover]` are
  invalidated at once.
- Reads that leave the start path (AO-6): full instance-store replay; workbench recovery; monitor
  registration scan (→ `monitor.journal`); `PolicyHost` folds by type; dispatch reconciliation and
  settlement projection; approval projection (→ the state document's projection); agent recovery;
  the previous epoch's settlement read (→ X1); planning signals by checkpoint (→ the state
  document). State migration reconciliation stays (X4).
- **Shutdown** is the reverse: business stops → close record through G16 → drain → close the
  ledger (§8.2).

## 13 Identity without ledger positions (AO-3, AO-4; AO-4q in v0.12.0)

- **Event identity**: the `EventId` exists at capture and may be linked at once. Business code
  never uses a ledger sequence.
- **Fact snapshot id**: `hash(context, instance store (lineage, revision), workbench (lineage,
  revision), records)` instead of `hash(context, ledger_position, records)`. Admission still
  compares it and refuses with `policy_facts_stale`.
- **Payload fields**: `policy.dispatch_intent` and `task.selection_evaluated` replace
  `input_ledger_position` with `input_fact_revisions` (both stores' (lineage, revision)). In
  v0.12.0 (order-v012 §3.5 item 11): ledger revision 3 keeps `input_ledger_position`; SH-1 adds the
  optional `input_fact_revisions`; readers accept either (`input_ledger_position`, when present,
  still non-zero); AO-3 starts writing the new field; AO-4 stops writing the old one and deletes
  the reads of the ledger head (`policy_dispatch.rs:813`, `:1156`, `:1488`; `policy_host.rs`
  admission comparison). This departs from LEDG §11's "change the field directly" by plan, and
  the shape is still registered once in v0.12.0.
- **Status source** (`RuntimeStateSource`, AO-4q, runtime-client revision 2): `{instance_facts,
  runtime_facts, ledger_committed_head, sampled_started_at_unix_ms,
  sampled_completed_at_unix_ms}`, where each store field is an optional `{lineage, revision}`
  (absent before AO-3; readers accept absence) and `ledger_committed_head` is the highest
  committed sequence (0 for an empty ledger). Status queries stop appending records (AO-4q).
  The UI reader changes with the UI's next version.
- **Receipts** of gate-class operations (inputs, leases, dispatch included) carry the event id and
  the store revisions, never a ledger sequence; install control receipts are sent after the commit
  and carry the event id too (AO-4; with runtime-client revision 3). The offline subcommands'
  receipts stay as today.
- History by ledger position (`fact_store.rs:474-498`) remains for actingledger forensics only.

**Wake source (AO-7)**: actingd stops subscribing to the whole ledger. The host sends business
notifications: store revisions changed → `FactsChanged`; lease release, transfer, expiry →
`ResourcesChanged`; catalog change → `CatalogChanged`; performance and resource changes →
`ResourcesChanged` (today's mapping, `main.rs:850-885`, with a business source). A slow or stopped
ledger never delays re-scoring.

## 14 Probes and visibility (AO-8; T8 with AO-1)

### 14.1 Timed probes

They run on one probe thread of the ledger module (or on the performance monitor's tick) and never
enter the writer queue. The ledger's memory part holds the queue's memory segment, the index
mirror (`state` rows: revision, value hash, sequence; `open`, `request`, `lifecycle` rows) and the
gate and probe counters.

| # | Probes | Reads | Period |
|---|---|---|---|
| T1 | stores and ledger agree (sampled) | both stores' current values (a bounded copy under the gate) against the index mirror, only revisions already committed | `probe_interval_ms` (60 s), `probe_sample_keys` |
| T2 | the ledger seals a snapshot itself | the same copy; only when a revision changed; record `runtime.fact_snapshot` (origin: the ledger) | with T1 |
| T3 | watermark and spill | memory bytes and records, tier, dump segment size | `health_probe_interval_ms` (1 s) |
| T4 | flush lag | age of the oldest unflushed record | with T3 |
| T5 | retries | flush thread counters | with T3 |
| T7 | silent gate | store revisions against captured revisions | with T1 |
| T8 | writer alive, attempt stalled | writer health; current attempt's start time; beyond `flush_attempt_stall_ms` → §9.1 | with T3 |

T8 as built by AO-1: a probe thread starts with every writer that has a failure plan and looks
at the attempt in progress every `health_probe_interval_ms`; the attempt's start is read from
the plan's monotonic clock. It holds the writer's queue weakly, so once the writer has ended
every later command fails at once as `writer_unavailable`.

- A finding is the record `ledger.health_observed` (the ledger's own record) with the finding's
  code: `ledger_store_mismatch` (error, needs a person; key, both revisions, both value hashes;
  neither side is changed, once per source and stretch), `ledger_store_revision_skipped`,
  `ledger_capture_buffer_busy`, `ledger_capture_spilled`, `ledger_capture_buffer_overloaded`,
  `ledger_flush_delayed`. Watermark, spill and lag findings are reported per segment: start when
  the activation line is crossed, end when the release line is crossed (with duration, count and
  bytes).
- No finding writes nothing; status only shows "latest probe: `ledger_probe_done`".
- "Needs a person" is a property of the code, never of a count.
- The ledger never scans the whole ledger for probes, start or idempotency (test: whole-ledger
  queries = 0).

### 14.2 Status blocks

`actingctl status` and MCP `ac_overview` gain two blocks, read from memory, writing nothing
(AO-8, with the UI-paired runtime-client revision):

- `ledger`: committed head, flush lag, tier and bytes, dump segment size and place, capture counts
  per gate, gaps in the last `status_gap_window_ms` and the latest gap, retry count, latest probe;
- `tagged_state`: this start's load (counts per source, records read, elapsed, load time L), the
  enabled marker, the last dump import.

### 14.3 Views

The six views stay (`event/views.rs`). The new record types join them through a view schema
upgrade in ledger revision 3 (`views.rs:40-43`), see [query conditions](global-ledger-query.md):
gap records and probe findings join Health (and Errors by severity); value records and the D2
transition record join Changes; `ledger.dump_imported` joins Health.

## 15 Configuration keys `ledger_observer.*`

A new section of the actingd configuration, in the one v0.12.0 revision of `actingd-config`
(revision 5, declared [5, 5] for v0.12.0 by #378 A8). Defaults go into the defaults file `actingd.defaults.json` of #387 D1b.
**Slice AO-1 brings parsing, validation and defaults of every key below in one PR** (order-v012
R3, R4), including the keys that later slices read; `config.parameters` reports each key's value
and source. `settable` follows #387 §5.5 and is fixed by AO-1 (proposal: watermark and batch
sizes `false` — the queue exists before the ledger opens; probe periods `true`).

| Key | Default | Type | Meaning | Read by (takes effect) |
|---|---|---|---|---|
| `buffer_bytes` | 64 MiB | integer | memory segment byte limit (hard limit = 100 %) | AO-2, v0.12.0 |
| `buffer_records` | 20 480 | integer | memory segment record limit | AO-2, v0.12.0 |
| `buffer.tier1_ratio` | 0.60 | decimal | tier 1 | AO-2, v0.12.0 |
| `buffer.tier2_ratio` | 0.75 | decimal | tier 2: start spilling | AO-2, v0.12.0 |
| `buffer.tier3_ratio` | 0.90 | decimal | tier 3: alarm, spill everything | AO-2, v0.12.0 |
| `buffer.hysteresis_ratio` | 0.10 | decimal | release line = tier × (1 − it) | AO-2, v0.12.0 |
| `spill_batch_bytes` | 4 MiB | integer | bytes per spill; tier3 − tier2 must leave at least this | AO-2, v0.12.0 |
| `dump_max_bytes` | 1 GiB | integer | dump file limit; reaching it → §9.2 | AO-1, v0.12.0 |
| `flush_batch_records` | 256 | integer | records per batch below tier 1 | AO-2, v0.12.0 |
| `flush_batch_records_busy` | 2 048 | integer | records per batch at tier 1 and above | AO-2, v0.12.0 |
| `write_attempts` | 5 | integer | attempts including the first | AO-1, v0.12.0 |
| `write_retry_backoff_ms` | [0, 100, 250, 500, 1000] | list of ms | wait before each attempt | AO-1, v0.12.0 |
| `flush_attempt_stall_ms` | 120 000 | ms | one attempt's stall limit (§9.1) | AO-1, v0.12.0 |
| `flush_lag_warn_ms` | 5 000 | ms | T4 threshold; validated below #375's settle age at load | AO-8, 0.12.x |
| `health_probe_interval_ms` | 1 000 | ms | T3, T4, T5, T8 period | AO-1 (T8), v0.12.0 |
| `probe_interval_ms` | 60 000 | ms | T1, T2, T7 period | AO-8, 0.12.x |
| `probe_sample_keys` | 1 024 | integer | keys per T1 sample: the whole instance store plus a workbench sample | AO-8, 0.12.x |
| `request_index_records` | 4 096 | integer | ring size of `request` rows (X3) | AO-2 (rows), AO-6 (read), v0.12.0 |
| `status_gap_window_ms` | 86 400 000 | ms | window of "recent gaps" in status | AO-8, 0.12.x |
| `tag.workbench_patterns` | `task.*.last_duration_ms`, `task.*.last_outcome`, `task.*.failure_streak`, `task.*.completed_at_unix_ms`, `host.scheduling_pause`, `host.install_transition`, `host.install_transition.pauses`, `host.perf_baseline.*`, `host.perf_calibration.*`, `host.config_overrides` | list | tagged workbench keys | AO-2 (index rows), AO-5 (rules), v0.12.0 |
| `tag.reobserve_prefixes` | `device.`, `backend.`, `application.`, `lease.`, `provider.`, `config.`, `task.page`, `task.game`, `task.server`, `host.pack_registry.` | list | workbench keys known to be untagged and re-observed | AO-5, 0.12.x |

- `settable` as fixed by AO-1: `flush_lag_warn_ms`, `probe_interval_ms`, `probe_sample_keys` and
  `status_gap_window_ms` are read where they are used (`true`); every other key is read when the
  ledger opens, before any instruction value is restored (`false`, "used before the ledger
  opens"), `health_probe_interval_ms` included, since the probe starts with the writer. The
  section's check refuses a broken rule as `config_value_invalid` (`section_check`) naming the key:
  the tiers rise strictly within (0, 1], the hysteresis lies within (0, 1), tier 3 − tier 2 of
  `buffer_bytes` holds at least `spill_batch_bytes`, `write_retry_backoff_ms` lists one wait per
  attempt, and the tag lists hold non-empty texts without spaces. Every integer and millisecond
  key is at least 1.
- `host.config_overrides` joins `tag.workbench_patterns` (Workflow #387 §5.7, handed to AO-0);
  `host.pack_registry.` joins `tag.reobserve_prefixes` (Workflow #389 §6.5); `config.selection`
  (#387) and `config.catalog` (MCP-9a) fall under the existing `config.` prefix.
- These defaults are a starting point; AO-2's first round fills them from a real-machine copy.
- Removed keys of model v1.1 never shipped: `decision_buffer_records`,
  `observation_buffer_records`, `buffer_warn_percent`, `exception_reserve_records`,
  `exception_wait_ms`, `admission_flush_wait_ms`, `degraded_retry_interval_ms`,
  `degraded_halt_after_ms`, `state_files.*`.

## 16 Outcome codes

Registered by AO-0 as `review: pending` in `contracts/outcome-codes/ledger.json` and in the
ledger registry `crates/ledger/src/codes.rs` (`LedgerObserverCode`), prefix `ledger_` (domain
unchanged). Nothing emits them before the slice named.

| Code | Category | Needs a person | Values (required; optional) | Slice |
|---|---|---|---|---|
| `ledger_write_retried` | warning | no | `attempts`; `io_kind`, `elapsed_ms` | AO-1 |
| `ledger_write_failed` | fatal | yes | `attempts`; `io_kind`, `io_op`, `timeout_ms`, `path` (the dump file) | AO-1 |
| `ledger_dump_imported` | info | no | `count`, `duplicate_count`, `dump_sealed`; `dump_sha256` | AO-1 |
| `ledger_dump_incomplete` | warning | no | `gap_start_unix_ms`, `gap_end_unix_ms`; `count` | AO-1 |
| `ledger_dump_invalid` | error | yes | `gap_start_unix_ms`, `gap_end_unix_ms`, `kept_path`; `count` | AO-1 |
| `ledger_dump_unsupported` | fatal | yes | `path`, `dump_version` | AO-1 |
| `ledger_dump_failed` | fatal | yes | `paths` (both places tried); `io_kind`, `io_op` | AO-1 |
| `ledger_tail_unconfirmed` | warning | no | `gap_start_unix_ms`, `gap_end_unix_ms`; `owner_epoch` | AO-2 |
| `ledger_record_rejected` | error | yes | `event_id`; `ledger_source`, `source_lineage`, `source_revision` | AO-2 |
| `ledger_store_revision_skipped` | error | yes | `ledger_source`, `source_lineage`, `expected_revision`, `source_revision` | AO-2 |
| `ledger_capture_buffer_busy` | warning | no | `segment_edge`; `count`, `buffer_bytes`, `elapsed_ms` | AO-2 |
| `ledger_capture_spilled` | warning | no | `segment_edge`; `count`, `buffer_bytes`, `elapsed_ms` | AO-2 |
| `ledger_capture_buffer_overloaded` | warning | yes | `segment_edge`; `count`, `buffer_bytes`, `elapsed_ms` | AO-2 |
| `ledger_flush_delayed` | warning | no | `segment_edge`, `elapsed_ms`; `timeout_ms` | AO-8 |
| `ledger_store_mismatch` | error | yes | `ledger_source`, `fact_key`; `source_revision`, `expected_revision`, `expected_sha256`, `actual_sha256` | AO-8 |
| `ledger_probe_done` | info (status only) | no | `at_unix_ms` | AO-8 |
| `ledger_tag_key_unclassified` | warning | no | `fact_key` | AO-5 |
| `ledger_tagged_state_seeded` | info | no | `count` | AO-5 |
| `ledger_tagged_state_loaded` | info | no | `count`, `elapsed_ms`; `event_count`, `source_counts` | AO-6 |
| `ledger_tagged_entries_dropped` | info | no | `count` | AO-6 |
| `ledger_tag_index_mismatch` | fatal | yes | `fact_key`; `event_id`, `expected_sha256`, `actual_sha256` | AO-6 |

- In `ledger_store_mismatch` and `ledger_tag_index_mismatch`, `expected_*` is the ledger index's
  side and `actual_*` the store's or the record's side; neither side is authoritative over the
  other (§14.1).
- `ledger_write_failed` sets `timeout_ms` (the configured `flush_attempt_stall_ms`) when an
  attempt stalled; `io_kind` is the last I/O failure's kind when there was one.
- New keys (catalog `keys`, mirrored in `outcome/keys.rs`): `duplicate_count`, `dump_sealed`,
  `dump_sha256`, `dump_version`, `gap_start_unix_ms`, `gap_end_unix_ms`, `ledger_source`
  (vocabulary `ledger_source`), `source_lineage`, `source_revision`, `expected_revision`,
  `segment_edge` (vocabulary `segment_edge`), `buffer_bytes`, `source_counts`
  (records of `ledger_source`, `count`). The two vocabularies have no `source` enum yet; the slice
  that builds them from an enum adds `source`.
- One code, one category: `ledger_dump_invalid` is only "middle hash mismatch, prefix imported,
  file kept" (error); a newer dump version is `ledger_dump_unsupported` (fatal).
- Reused codes, registered by #378 A2: `writer_unavailable`, `writer_response_timeout` (error, no
  longer fatal), `policy_facts_stale`. `runtime_halted` is not used.

## 17 Iron rules (Workflow `Codex/IRON-RULES.md`, changed by Alice; wording approved 10-09 22:1x)

- **Rule 3** becomes:
  > 账本是对外唯一的权威记录。也是对内的历史权威记录，实时状态归两块内存库。
  > 账本里写的就是 Runtime 发生过的事；漏记以缺口记录写明。两块内存库（调度器工作台、实例基础信息库）不对外充当历史。
  > Runtime 本体经捕获接口交出记录；只有登记在例外表、探针抓不到的记录直接写入。
  > 账本写入失败先立即重试；用尽后把未落盘的记录写进临时转储文件，以致命故障停止；下次启动先把转储计入账本，再移除转储。账本完整性故障是程序故障。
- **Rule 13** becomes:
  > 账本是唯一的历史落盘。两块内存库不另设文件：需要跨重启的决策与基础信息，是账本里带标签的记录；标签放在账本库的索引表里，与记录同一事务维护；启动时只读这张表及其指向的记录载入内存。
  > 本条之外的落盘还有：配置输入、厂商工具、owner lock、`runtime-state.sqlite` 的状态文档、`monitor.journal`、帧文件与 `kept\.moves`（#375）、看门狗的 `state.json`（#374），以及账本写入失败或溢写时的临时转储文件（导入后即删）。
- The Chinese mirror `Codex/铁律.md` changes the same way. By rule 11 only Alice decides
  exceptions to the iron rules.

## 18 Slices

| Slice | Content | Depends on | Release |
|---|---|---|---|
| AO-0 | this documentation, the register, the codes, the call-site inventory | frozen model | — |
| AO-1 | §9 (retry, read back, dump, fatal, import, stall); every `ledger_observer.*` key parsed | AO-0, #387 D1b, A3, SH-1 | v0.12.0 |
| AO-2 | §2.1 implementation, §7, §7.1, §8, gap records, `ledger_index` maintained from the new ledger's first record, flush barrier, close drain, tier read handle | AO-1; the watermark extraction after #664 | v0.12.0 |
| AO-3 | §3; the instance store's single entry, revisions and lineages, snapshot ids by revision pair; §4.1a direct writes through `capture`; G16; the architecture guard (Appendix A as the starting whitelist); the ladder's direction-1 counter moves into `capture` | AO-2 | 0.12.x, before #367 |
| AO-4 | workbench accepts first; decision memory and value records; status queries stop writing (AO-4q in v0.12.0); core stops sealing snapshots; payload fields by revision pair; `execute_critical` stops waiting; G13, G14, G15, G3 receipts; `artifact.verified` decision and X6; B-class choke points; settlement continuation retired | AO-3 | 0.12.x (AO-4q: v0.12.0) |
| AO-5 | tag rules, unclassified warning, seeding once, shadow comparison in tests | AO-4 | 0.12.x, same release as AO-6 |
| AO-6 | §11.5, §12; X1–X3 from the index; X6; start replay stops | AO-5 | 0.12.x |
| AO-7 | §13 wake source | — | v0.12.0 |
| AO-8 | §14; status and MCP blocks | AO-2 (T1, T2 also AO-3, AO-4) | 0.12.x |
| AO-9 | views, MCP run state, actingledger gap and import queries, #375 gap rule | AO-2, AO-8 | 0.12.x |

Order: AO-0 → AO-1 → AO-2 → AO-3 → AO-4 → AO-5 + AO-6 → AO-8 → AO-9; AO-7 in parallel with AO-1.
The test rows (AO1-a … AO9-d) are LEDG §14.2 and stay the requirement, except AO6-n and AO9-d,
which the no-compatibility ruling voids.

## Appendix A: call-site inventory (`main` 706ead94)

AO-0's survey of every ledger write in the non-test code of `runtime-host` and `actingd`. In A.1–A.7
a bare `§n` is a section of LEDG (the model); in A.8 it is a section of this contract. The table is
the starting whitelist of AO-3's guard and the work list of AO-3 and AO-4.

### A.1 Scope, method, legend

- **Scope**: non-test code of `crates/runtime-host/src` and `apps/actingd/src` (§4.8 guard scope). Excluded as test code: `tests/` directories, `tests.rs` / `*_tests.rs` / `test_*.rs`, inline `#[cfg(test)]` items, functions named `*_for_test`, and `apps/actingd/src/main.rs` from `mod tests` (line 1884).
- **Method**: `git grep -n -E "\.(append[a-z_]*|execute_critical|reconcile_[a-z_]*)\s*\("` and `git grep -n -E "fn (append[a-z_]*|execute_critical)"` as the seed, widened to free-function calls (`execute_critical(`, `append_runtime_start_event(` …), `record_runtime_fact*`, `invalidate_runtime_fact`, the kernel `record(trace)` host impls, the two `ArtifactEventSink` impls, `ProviderStartup::record/failed`, `LedgerMaintenance::import`, and every other `GlobalLedger` method that writes (`release_lab_pin`, `admit_artifact_eviction`, `finish_artifact_eviction`, `reconcile_prior_epoch_closes`, `reconcile_scheduled_policy_settlement`). Receivers were read to drop non-ledger `append`s. Event types come from the payload constructor at each site (or the wrapper body); the survey scripts are AO-0 scratch, not committed.
- **Row unit**: one row per textual call or wrapper definition. Business functions that merely end in an append (`publish_facts`, `record_performance_events`, `record_lifecycle_result`, `record_required_failure*`, `record_scheduling_pauses`, `record_install_value` …) are represented by the append rows inside them; their own callers are not separate rows. Exception: the `reconcile_*` host functions are listed because the §4.8 guard pattern matches their names.
- **Excluded after reading the receiver** (not ledger writes): `rh/agent_dispatcher.rs:403` and `rh/host/performance.rs:377` (`Vec::append`); `rh/owner.rs:244/273/302/349/380/516/622` (owner journal file, `append_in_epoch` / `append_record`); `rh/host/task_diagnostic.rs:169/170/259/873` (diagnostic file stream); `rh/host.rs:1130` `LedgerMaintenance::initialize_empty` (creates the schema and the meta row, no event record); `rh/host/device_diagnostic.rs:220` `GlobalLedger::confirm_deferred` (commits deferred appends, writes no record — today's analogue of the §6.2 barrier).

Legend:

- `rh/` = `crates/runtime-host/src/`, `ad/` = `apps/actingd/src/`. Model abbreviations kept: `f` = `rh/host/facts.rs`, `rf` = `rh/host/runtime_facts.rs`, `ct` = `rh/host/contained_task.rs`, `ls` = `rh/host/lease.rs`, `pd` = `rh/host/policy_dispatch.rs`, `po` = `rh/host/policy_outcome.rs`, `h` = `rh/host.rs`, `hp` = `rh/host/performance.rs`, `pl` = `rh/host/planning.rs`, `g` = `crates/ledger/src/global.rs`.
- `api`: the ledger API or wrapper called at the site; `(def)` = wrapper definition. `GlobalLedger::*` / `execute_critical` / `LedgerMaintenance::import` = direct ledger calls.
- `writes`: event type(s); a `runtime.lifecycle_observed` row names its phase in backticks after the type. "varies (caller's draft)" for the generic `f` wrappers.
- `class`: A business event, B ledger-only record, W exception table (§4.1b). "per caller" / "per phase" only on generic wrapper rows.
- `gate`: G1–G16 per §4.1; W1/W2 per §6.1; `T2` = ledger self-seal (§6.1); `stop` = the model stops the write; `—` = wrapper definition or forward only; a trailing `?` = proposed gate for a row the model does not place (see §4).
- `category`: decision = today's durable class, observation = today's observational class (`contracts/ledger-store.md:71-119`), except `artifact.verified` → decision (§3.6 / G9). Instance-scoped `runtime.fact_*` are observational today; Runtime-scoped are durable ("per scope").
- Note tokens: **DIRECT** = calls the ledger directly, bypassing `f:511-557`; **UFG** = one of the 14 `append_event_under_fact_gate` calls (caller already holds `fact_write_gate`); **SU** = runs in the startup segment before business opens (§10.1); **D1c** = record family that gains the recovery-ladder direction-1 counter (`model-387-ladder-config.md` §6.4 item 6; backend open, self-check, capture); **seq→receipt** = the returned `PersistedEvent` becomes a `TerminalEvent {event_id, sequence}` in a receipt (`requests.rs:1345`); **seq→projection** = the persisted event is applied to an in-memory store that keeps its sequence; **seq used** = the sequence is read directly. All three are §3.4 change points ("uses the returned ledger sequence"). Settlement continuation = `g:1599` (`global.rs:1706` at HEAD).

### A.2 Table

| # | site | fn | api | writes | class | gate | category | note |
|---|---|---|---|---|---|---|---|---|
| 1 | `rh/host.rs:1260` | `RuntimeHost::start_with_provider` | `GlobalLedger::reconcile_prior_epoch_closes` | runtime.lifecycle_observed `prior_epoch_owner_imported` / `prior_epoch_scope_closed` (drafted inside the ledger writer, `retention/prior_epoch.rs:335`) | B | ledger-own? | decision | DIRECT; SU; host triggers `GlobalLedger::reconcile_prior_epoch_closes`; misfit M3; not in §4.1a |
| 2 | `rh/host.rs:1363` | `RuntimeHost::start_with_provider` | `append_device_diagnostic_record` | runtime.lifecycle_observed `device_diagnostic_summary` | B | G16 | decision | aborted-start summary via `append_deferred` + `confirm_deferred`; SU |
| 3 | `rh/host.rs:1523` | `RuntimeHost::start_with_provider` | `append_lifecycle_observed` | runtime.lifecycle_observed `prior_epoch_owner_released_by_exit` | A | G16 | decision | SU; process lifecycle |
| 4 | `rh/host.rs:1631` | `RuntimeHost::start_with_provider` | `reconcile_policy_dispatches` | policy.execution_recorded + policy.dispatch_completed (continuation), policy.dispatch_rejected | A | G5 | decision | SU; settlement continuation (`g:1599`→`global.rs:1706`) via po:1687 |
| 5 | `rh/host.rs:1663` | `RuntimeHost::start_with_provider` | `reconcile_runtime_state` | state.migrated, release.staged (+ state document) | A | G15 | decision | SU; install family joint transaction |
| 6 | `rh/host.rs:1697` | `RuntimeHost::start_with_provider` | `reconcile_agent_wakes` | agent.wake_requested | A | G12 | decision | SU |
| 7 | `rh/host.rs:1713` | `RuntimeHost::start_with_provider` | `append_runtime_start_event` | runtime.started / runtime.takeover | A | G16 | decision | SU; model `h:1543` |
| 8 | `rh/host.rs:1720` | `RuntimeHost::start_with_provider` | `append_instance_binding_events` | runtime.instance_bound | A | G16 | decision | SU; model `h:1550` |
| 9 | `rh/host.rs:3289` | `RuntimeHost::record_lifecycle_failure` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | public `RuntimeHost::record_lifecycle_failure`, called by actingd (ad/main.rs:1809) |
| 10 | `rh/host.rs:3337` | `RuntimeHost::shutdown` | `append_lifecycle_observed` | runtime.lifecycle_observed `shutdown_requested` | A | G16 | decision | close path; model `h:3029-3048` |
| 11 | `rh/host/agent_control.rs:21` | `expire_agent_sessions` | `append_event_raw` | agent.session_escalated | A | G12 | decision | seq→receipt; seq→projection (apply) |
| 12 | `rh/host/agent_control.rs:56` | `start_agent_session` | `append_event` | agent.session_started | A | G12 | decision | seq→receipt; seq→projection (apply) |
| 13 | `rh/host/agent_control.rs:101` | `resume_agent_session` | `append_event` | agent.session_resumed | A | G12 | decision | seq→receipt; seq→projection (apply) |
| 14 | `rh/host/agent_control.rs:178` | `record_agent_response` | `append_event` | agent.response_recorded / agent.session_completed / agent.session_escalated | A | G12 | decision | seq→receipt; seq→projection (apply) |
| 15 | `rh/host/agent_control.rs:221` | `reconcile_agent_wakes` | `reconcile_agent_wakes` (def) | agent.wake_requested | A | — | decision | wrapper (def); reads the ledger at startup to rebuild wake obligations |
| 16 | `rh/host/agent_control.rs:274` | `reconcile_agent_wakes` | `append_agent_wake` | agent.wake_requested | A | G12 | decision | wake trigger carries the source event's sequence (`agent_control.rs:295`) |
| 17 | `rh/host/agent_control.rs:279` | `append_agent_wake` | `append_agent_wake` (def) | agent.wake_requested | A | — | decision | wrapper (def) |
| 18 | `rh/host/agent_control.rs:324` | `append_agent_wake` | `GlobalLedger::append` | agent.wake_requested | A | G12 | decision | DIRECT; not in §4.1a; result applied to the dispatcher (stores `event_sequence`) |
| 19 | `rh/host/backend_open.rs:8` | `append_backend_open_failure_observations` | `append_backend_open_failure_observations` (def) | runtime.lifecycle_observed `backend_open_observed` | B | — | observation | wrapper (def); D1c |
| 20 | `rh/host/backend_open.rs:15` | `append_backend_open_failure_observations` | `append_backend_open_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | D1c |
| 21 | `rh/host/backend_open.rs:29` | `append_backend_open_observations` | `append_backend_open_observations` (def) | runtime.lifecycle_observed `backend_open_observed` | B | — | observation | wrapper (def); D1c |
| 22 | `rh/host/backend_open.rs:51` | `append_backend_open_observations` | `append_event_under_fact_gate` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | UFG; D1c (failure Error / retried Warning) |
| 23 | `rh/host/client_events.rs:11` | `release_lab_pin` | `append_event` | lab.request (pin release) | A | G14 | decision | write request record; its `TerminalEvent` (with sequence) is handed to `release_lab_pin` at `:21` |
| 24 | `rh/host/client_events.rs:21` | `release_lab_pin` | `GlobalLedger::release_lab_pin` | artifact.pin_released (drafted inside the ledger writer) | B | G9 | decision | DIRECT (`GlobalLedger::release_lab_pin`); not in §4.1a; misfit M4 |
| 25 | `rh/host/client_events.rs:63` | `record_authoring_event` | `append_event` | resource.* (authoring phase) | A | G14 | decision | consumer: actingd wake (ResourceAuthoring family → ResourcesChanged); seq→receipt |
| 26 | `rh/host/client_events.rs:113` | `record_debug_event` | `append_event` | lab.request | B | G14 | decision | Lab-reported debug audit (no business reader) |
| 27 | `rh/host/client_events.rs:121` | `record_debug_event` | `append_event` | task.step_finished | B | G7 | decision | Lab debug run trace |
| 28 | `rh/host/client_events.rs:129` | `record_debug_event` | `append_event` | task.completed | B | G7 | decision | Lab debug run terminal |
| 29 | `rh/host/client_events.rs:137` | `record_debug_event` | `append_event` | task.failed | B | G7 | decision | Lab debug run terminal |
| 30 | `rh/host/client_events.rs:150` | `record_debug_event` | `append_event` | command.validated | B | G14 | decision | Lab-reported debug audit |
| 31 | `rh/host/client_events.rs:162` | `record_debug_event` | `append_event` | command.rejected | B | G14 | decision | Lab-reported debug audit |
| 32 | `rh/host/client_events.rs:250` | `record_client_action` | `append_event` | client.action | A | G14 | decision | idempotency reader `client_events.rs:265-300`; seq→receipt |
| 33 | `rh/host/contained_task.rs:1045` | `EntryRecoveryRuntime::record` | `ContainedTaskRuntime::record(trace)` (def) | task.* (kernel trace, entry-recovery subset) | B | — | per type | kernel `record(trace)` host impl (def), model `ct:919` |
| 34 | `rh/host/contained_task.rs:1054` | `EntryRecoveryRuntime::record` | `ContainedTaskRuntime::record(trace)` | task.* (kernel trace) | B | G7 | per type | forwards to the inner `RuntimeContainedTask::record` |
| 35 | `rh/host/contained_task.rs:1296` | `RuntimeContainedTask::record_task_geometry` | `append_event` | task.geometry_observed | B | G7 | observation | written after the deadline outside `record_entry_fact` |
| 36 | `rh/host/contained_task.rs:1549` | `RuntimeContainedTask::record_geometry_triggered_recovery_failure` | `append_task` | task.entry_preflight (entry recovery failed / target disposition) | B | G7 | decision |  |
| 37 | `rh/host/contained_task.rs:1589` | `RuntimeContainedTask::record_geometry_triggered_prerequisite_failure` | `append_task` | task.entry_preflight (prerequisite failure facts) | B | G7 | decision |  |
| 38 | `rh/host/contained_task.rs:2006` | `RuntimeContainedTask::record_unwritten_selection` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | unwritten run-ending selection record; operation-level stage |
| 39 | `rh/host/contained_task.rs:2017` | `RuntimeContainedTask::append_task` | `append_task` (def) | task.* | B | — | per type | wrapper (def), model `ct:1891` |
| 40 | `rh/host/contained_task.rs:2024` | `RuntimeContainedTask::append_task` | `append_event` | task.* | B | G7 | per type | wrapper body |
| 41 | `rh/host/contained_task.rs:2049` | `RuntimeContainedTask::record_entry_fact` | `append_task` | task.entry_preflight | B | G7 | decision | entry facts (recognition, recovery decision/admission/completion/failure, disposition) |
| 42 | `rh/host/contained_task.rs:2857` | `RuntimeContainedTask::capture` | `append_event` | capture.requested | B | G8 | observation | result's sequence feeds the effective-configuration record (`ct:3068` `source_sequence`); D1c |
| 43 | `rh/host/contained_task.rs:2911` | `RuntimeContainedTask::capture` | `append_backend_open_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | D1c |
| 44 | `rh/host/contained_task.rs:3089` | `RuntimeContainedTask::capture` | `append_backend_open_failure_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | D1c |
| 45 | `rh/host/contained_task.rs:3172` | `RuntimeContainedTask::capture` | `append_event` | capture.failed | B | G8 | observation | D1c (capture failure); seq→receipt |
| 46 | `rh/host/contained_task.rs:3380` | `RuntimeContainedTask::record` | `ContainedTaskRuntime::record(trace)` (def) | task.* (kernel trace) | B | — | per type | kernel `record(trace)` host impl (def), model `ct:3254` |
| 47 | `rh/host/contained_task.rs:3386` | `RuntimeContainedTask::record` | `ContainedTaskRuntime::record(trace)` | task.selection_evaluated (run-ending select record) | B | G7 | decision | recursive call for the run-ending selection |
| 48 | `rh/host/contained_task.rs:3413` | `RuntimeContainedTask::record` | `append_task` | task.requested | B | G7 | decision |  |
| 49 | `rh/host/contained_task.rs:3435` | `RuntimeContainedTask::record` | `append_task` | task.started | B | G7 | decision |  |
| 50 | `rh/host/contained_task.rs:3498` | `RuntimeContainedTask::record` | `append_event` | capture.completed | B | G8 | observation | D1c (capture) |
| 51 | `rh/host/contained_task.rs:3513` | `RuntimeContainedTask::record` | `append_task` | task.evidence_indexed | B | G7 | observation |  |
| 52 | `rh/host/contained_task.rs:3587` | `RuntimeContainedTask::record` | `append_event` | recognition.requested | B | G8 | observation |  |
| 53 | `rh/host/contained_task.rs:3598` | `RuntimeContainedTask::record` | `append_task` | task.recognition_started | B | G7 | observation |  |
| 54 | `rh/host/contained_task.rs:3652` | `RuntimeContainedTask::record` | `append_event_observed` | recognition.completed | B | G8 | observation |  |
| 55 | `rh/host/contained_task.rs:3677` | `RuntimeContainedTask::record` | `append_event_observed` | task.recognition_completed | B | G7 | observation |  |
| 56 | `rh/host/contained_task.rs:3756` | `RuntimeContainedTask::record` | `append_task` | task.step_started | B | G7 | decision |  |
| 57 | `rh/host/contained_task.rs:3794` | `RuntimeContainedTask::record` | `append_task` | task.selection_evaluated | B | G7 | decision |  |
| 58 | `rh/host/contained_task.rs:3854` | `RuntimeContainedTask::record` | `append_task` | task.effect_intent | B | G7 | decision |  |
| 59 | `rh/host/contained_task.rs:3872` | `RuntimeContainedTask::record` | `append_event_observed` | task.effect_completed | B | G7 | decision |  |
| 60 | `rh/host/contained_task.rs:3912` | `RuntimeContainedTask::record` | `append_task` | task.step_finished | B | G7 | decision |  |
| 61 | `rh/host/contained_task.rs:4034` | `RuntimeContainedTask::record` | `append_task` | task.terminal_intent | B | G7 | decision |  |
| 62 | `rh/host/contained_task.rs:4085` | `RuntimeArtifactEventSink::append` | `ArtifactEventSink::append` (def) | artifact.created, artifact.verified, artifact.store_failed / verification_failed / export_* | B | — | decision (created, verified); others per type | `ArtifactEventSink` impl (def), model `ct:3946-3960`; 14 construction sites (§7) |
| 63 | `rh/host/contained_task.rs:4093` | `RuntimeArtifactEventSink::append` | `GlobalLedger::append` | artifact.* | B | G9 | per type | DIRECT; model `ct:3964` |
| 64 | `rh/host/contained_task.rs:4766` | `run_contained_task` | `append_request_lifecycle` | cli.command / lab.request / ui.action, command.received, command.validated | A | G14 | decision | write request (task run) |
| 65 | `rh/host/contained_task.rs:5096` | `prepare_package_run` | `append_scheduled_request_lifecycle` | command.received, command.validated (scheduled) | A | G14 | decision | scheduler-originated request record |
| 66 | `rh/host/contained_task.rs:5423` | `run_scheduled_contained_task` | `append_scheduled_request_lifecycle` | command.received, command.validated (scheduled) | A | G14 | decision | scheduler-originated request record |
| 67 | `rh/host/contained_task.rs:5742` | `execute_contained_task_with_lease` | `append_contained_task_terminal` | capture.summary_committed, task.terminal_intent, task.completed/failed/cancelled | B | G7 | decision | receipt TerminalEvent carries the sequence |
| 68 | `rh/host/contained_task.rs:5820` | `execute_contained_task_with_lease` | `append_contained_task_terminal` | capture.summary_committed, task.terminal_intent, task.completed/failed/cancelled | B | G7 | decision | receipt TerminalEvent carries the sequence |
| 69 | `rh/host/contained_task.rs:5894` | `execute_contained_task_with_lease` | `append_contained_task_terminal` | capture.summary_committed, task.terminal_intent, task.completed/failed/cancelled | B | G7 | decision | receipt TerminalEvent carries the sequence |
| 70 | `rh/host/contained_task.rs:6026` | `execute_contained_task_with_lease` | `append_contained_task_terminal` | capture.summary_committed, task.terminal_intent, task.completed/failed/cancelled | B | G7 | decision | receipt TerminalEvent carries the sequence |
| 71 | `rh/host/contained_task.rs:6165` | `run_preflighted_contained_task` | `append_event` | runtime.failed (resource declaration) | B | G7 | decision | resource declaration rejected at preflight; receipt carries the sequence |
| 72 | `rh/host/contained_task.rs:6531` | `recover_contained_task` | `append_recovered_contained_task_terminal` | task.terminal_intent, task.completed/failed/cancelled (recovered) | B | G7 | decision | restart recovery of a direct run (X1); receipt carries the recovered terminal's sequence |
| 73 | `rh/host/contained_task.rs:6592` | `recover_contained_task` | `append_recovered_contained_task_release` | lease.released (recovered) | A | G4 | decision | restart recovery (X1) |
| 74 | `rh/host/contained_task.rs:6658` | `append_recovered_contained_task_terminal` | `append_recovered_contained_task_terminal` (def) | task.terminal_intent, task.completed/failed/cancelled (recovered) | B | — | decision | wrapper (def) |
| 75 | `rh/host/contained_task.rs:6677` | `append_recovered_contained_task_terminal` | `append_event_under_fact_gate` | task.terminal_intent | B | G7 | decision | UFG |
| 76 | `rh/host/contained_task.rs:6693` | `append_recovered_contained_task_terminal` | `append_event_under_fact_gate` | task.completed/failed/cancelled | B | G7 | decision | UFG; `contained_task_recovered_after_restart` |
| 77 | `rh/host/contained_task.rs:6723` | `append_recovered_contained_task_release` | `append_recovered_contained_task_release` (def) | lease.released (recovered) | A | — | decision | wrapper (def) |
| 78 | `rh/host/contained_task.rs:6767` | `append_recovered_contained_task_release` | `append_event` | lease.released | A | G4 | decision |  |
| 79 | `rh/host/contained_task.rs:6966` | `settle_previous_epoch_contained_run` | `append_recovered_contained_task_terminal` | task.terminal_intent, task.completed/failed/cancelled (recovered) | B | G7 | decision | previous-epoch settlement (X1); SU |
| 80 | `rh/host/contained_task.rs:7003` | `settle_previous_epoch_contained_run` | `append_recovered_contained_task_release` | lease.released (recovered) | A | G4 | decision | previous-epoch settlement (X1) |
| 81 | `rh/host/contained_task.rs:7154` | `append_contained_task_terminal` | `append_contained_task_terminal` (def) | capture.summary_committed, task.terminal_intent, task.completed/failed/cancelled | B | — | decision | wrapper (def), model `ct:7069` (`TaskTerminal`) |
| 82 | `rh/host/contained_task.rs:7237` | `append_contained_task_terminal` | `append_event_under_fact_gate` | capture.summary_committed | B | G7 | decision | UFG; capture family written at the terminal choke point |
| 83 | `rh/host/contained_task.rs:7250` | `append_contained_task_terminal` | `append_event_under_fact_gate` | task.terminal_intent | B | G7 | decision | UFG |
| 84 | `rh/host/contained_task.rs:7300` | `append_contained_task_terminal` | `append_event_under_fact_gate` | task.completed/failed/cancelled | B | G7 | decision | UFG; terminal sequence returned (receipt, settlement) |
| 85 | `rh/host/contained_task.rs:7395` | `check_contained_task_terminal` | `append_event_under_fact_gate` | task.terminal_rejected | B | G7 | decision | UFG |
| 86 | `rh/host/device_diagnostic.rs:102` | `append_device_diagnostic_record_under_fact_gate` | `append_device_diagnostic_record_under_fact_gate` (def) | runtime.lifecycle_observed `device_diagnostic_summary` | B | — | decision | wrapper (def) |
| 87 | `rh/host/device_diagnostic.rs:110` | `append_device_diagnostic_record_under_fact_gate` | `append_device_diagnostic_record` | runtime.lifecycle_observed `device_diagnostic_summary` | B | G16 | decision | then `confirm_deferred` (`:220`) |
| 88 | `rh/host/device_diagnostic.rs:148` | `close_device_diagnostic_budget` | `append_device_diagnostic_record_under_fact_gate` | runtime.lifecycle_observed `device_diagnostic_summary` | B | G16 | decision | close boundary; model `h:3752` → `host.rs:4066` |
| 89 | `rh/host/device_diagnostic.rs:187` | `append_device_diagnostic_record` | `append_device_diagnostic_record` (def) | runtime.lifecycle_observed `device_diagnostic_summary` | B | — | decision | wrapper (def) |
| 90 | `rh/host/device_diagnostic.rs:205` | `append_device_diagnostic_record` | `GlobalLedger::append_deferred` | runtime.lifecycle_observed `device_diagnostic_summary` | B | G16 | decision | DIRECT (`append_deferred`); listed in §4.1a |
| 91 | `rh/host/emulator_instance.rs:259` | `control_emulator_instance` | `append_client_command_intent` | cli.command / lab.request / ui.action, command.received | A | G13 | decision | emulator start/stop intent (via G14 request record) |
| 92 | `rh/host/emulator_instance.rs:452` | `drive_restart_phase` | `append_event` | command.received | A | G13 | decision | restart phase intent |
| 93 | `rh/host/emulator_instance.rs:707` | `drive_control_phase` | `append_event` | command.validated | A | G13 | decision | start/stop result; receipt carries the sequence |
| 94 | `rh/host/emulator_instance.rs:721` | `drive_control_phase` | `append_event` | runtime.instance_bound | A | G13 | decision | rebinding after start |
| 95 | `rh/host/emulator_instance.rs:934` | `record_stop_process_unverified` | `append_event` | runtime.failed | B | G13 | decision | stop not verified |
| 96 | `rh/host/emulator_instance.rs:989` | `apply_failure_observation` | `append_event_raw` | runtime.instance_bound | A | G13 | decision | observed port change |
| 97 | `rh/host/emulator_instance.rs:1011` | `record_observed_stopped` | `record_runtime_fact` | runtime.fact_recorded (`device.connected` = false) | A | G3 | observation | instance scope |
| 98 | `rh/host/emulator_instance.rs:1204` | `record_device_connected` | `record_runtime_fact` | runtime.fact_recorded (`device.connected`) | A | G3 | observation | instance scope |
| 99 | `rh/host/emulator_instance.rs:1237` | `invalidate_present_fact` | `invalidate_runtime_fact` | runtime.fact_invalidated (`device_closed`) | A | G3 | observation | instance scope |
| 100 | `rh/host/emulator_instance.rs:1263` | `emulator_control_failure` | `append_event` | command.rejected | A | G13 | decision | start/stop failure; receipt carries the sequence |
| 101 | `rh/host/evidence_export.rs:64` | `export_evidence` | `append_event` | task.terminal_intent | B | G7 | decision | debug run terminal intent |
| 102 | `rh/host/evidence_export.rs:72` | `export_evidence` | `append_event` | task.completed / task.failed / task.cancelled | B | G7 | decision | debug run terminal |
| 103 | `rh/host/facts.rs:352` | `publish_facts` | `append_event_under_fact_gate` | fact.published | A | G2 | decision | UFG; returns `FactPublication { sequence }` |
| 104 | `rh/host/facts.rs:453` | `append_event` | `append_event` (def) | varies (caller's draft) | per caller | — | per type | `f` wrapper (def) |
| 105 | `rh/host/facts.rs:462` | `append_event` | `append_event_raw` | varies (caller's draft) | per caller | — | per type | forward |
| 106 | `rh/host/facts.rs:466` | `append_event_observed` | `append_event_observed` (def) | varies (caller's draft) | per caller | — | per type | `f` wrapper (def) |
| 107 | `rh/host/facts.rs:480` | `append_event_observed` | `append_event_raw_with_observation` | varies (caller's draft) | per caller | — | per type | forward |
| 108 | `rh/host/facts.rs:492` | `append_event_raw` | `append_event_raw` (def) | varies (caller's draft) | per caller | — | per type | `f` wrapper (def) |
| 109 | `rh/host/facts.rs:501` | `append_event_raw` | `append_event_raw_with_observation` | varies (caller's draft) | per caller | — | per type | forward |
| 110 | `rh/host/facts.rs:511` | `append_event_raw_with_observation` | `append_event_raw_with_observation` (def) | varies (caller's draft) | per caller | G1 | per type | `f` wrapper (def) = today's fan-out `f:511-557`; pipeline observation `f:552` |
| 111 | `rh/host/facts.rs:532` | `append_event_raw_with_observation` | `append_event_under_fact_gate_with_observation` | varies (caller's draft) | per caller | G1 | per type | forward under the gate |
| 112 | `rh/host/facts.rs:560` | `append_event_under_fact_gate` | `append_event_under_fact_gate` (def) | varies (caller's draft) | per caller | — | per type | `f` wrapper (def), caller already holds the gate |
| 113 | `rh/host/facts.rs:569` | `append_event_under_fact_gate` | `append_event_under_fact_gate_with_observation` | varies (caller's draft) | per caller | — | per type | forward |
| 114 | `rh/host/facts.rs:579` | `append_event_under_fact_gate_with_observation` | `append_event_under_fact_gate_with_observation` (def) | varies (caller's draft) | per caller | G1 | per type | common inner layer (def); planned D1c hook site (ladder model §3.4, not in HEAD); also feeds the device diagnostic budget (`:628`) |
| 115 | `rh/host/facts.rs:612` | `append_event_under_fact_gate_with_observation` | `GlobalLedger::append_with_observation` | varies (caller's draft) | per caller | G1 | per type | DIRECT ledger call of the `f` layer (observed variant) |
| 116 | `rh/host/facts.rs:616` | `append_event_under_fact_gate_with_observation` | `GlobalLedger::append` | varies (caller's draft) | per caller | G1 | per type | DIRECT ledger call of the `f` layer |
| 117 | `rh/host/facts.rs:666` | `synchronize_fact_store_under_gate` | `GlobalLedger::append_transaction` | fact.invalidated (derived) + state tombstone projection | A | G2 | decision | DIRECT (`append_transaction`); listed in §4.1a; sequence acknowledged into the store |
| 118 | `rh/host/failure_settlement.rs:341` | `record_scheduled_policy_diagnostic` | `append_event_raw` | runtime.failed | B | G5 | decision | scheduled-policy diagnostic |
| 119 | `rh/host/foreground_gate.rs:131` | `reject_input` | `append_event` | command.rejected | A | G14 | decision | input refused by the foreground gate; receipt sequence |
| 120 | `rh/host/foreground_gate.rs:170` | `record_foreground_application` | `record_runtime_fact` | runtime.fact_recorded (application foreground) | A | G3 | observation | instance scope |
| 121 | `rh/host/foreground_gate.rs:193` | `invalidate_adb_baseline` | `invalidate_runtime_fact` | runtime.fact_invalidated (ADB baseline) | A | G3 | observation | instance scope |
| 122 | `rh/host/frame_retention.rs:120` | `record_frame_retention_warning` | `append_event_raw` | runtime.failed | B | G9 | decision | frame retention warning |
| 123 | `rh/host/frame_retention.rs:210` | `pin_published_frames` | `GlobalLedger::append` | artifact.pin_recorded | B | G9 | observation | DIRECT; listed in §4.1a (model `frame_retention.rs:156`) |
| 124 | `rh/host/frame_retention.rs:702` | `EvictionRecovery::round` | `GlobalLedger::admit_artifact_eviction` | artifact.eviction_intent (drafted inside the ledger writer) | B | ledger-own? | decision | DIRECT; startup eviction recovery; misfit M3; not in §4.1a |
| 125 | `rh/host/frame_retention.rs:709` | `EvictionRecovery::round` | `GlobalLedger::finish_artifact_eviction` | artifact.eviction_outcome (drafted inside the ledger writer) | B | ledger-own? | decision | DIRECT; startup eviction recovery; misfit M3; not in §4.1a |
| 126 | `rh/host/governance.rs:97` | `record_approval_decision` | `GlobalLedger::append_transaction` | approval.decision + approval projection | A | G15 | decision | DIRECT (`append_transaction`); listed in §4.1a; receipt + projection keyed by sequence |
| 127 | `rh/host/governance.rs:298` | `declare_governance_identity` | `append_event` | governance.identity_declared | B | G14 | decision | connection set is in memory; record has no reader; seq→receipt |
| 128 | `rh/host/host_claims.rs:209` | `record_worker_panic` | `append_event_raw` | runtime.failed | B | G16 | decision | worker panic |
| 129 | `rh/host/host_claims.rs:405` | `release_host_key` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | host key release failure |
| 130 | `rh/host/input.rs:126` | `input` | `append_scheduler_admitted_for_token` | scheduler.admitted | A | G4 | decision |  |
| 131 | `rh/host/input.rs:199` | `input` | `execute_critical` | input.intent, input.committed / input.failed | A | G6 | decision | DIRECT (`execute_critical`); listed in §4.1a (model `input.rs:197`); receipt carries the outcome sequence |
| 132 | `rh/host/input.rs:241` | `input` | `append_backend_open_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | inside the critical action; D1c |
| 133 | `rh/host/input.rs:253` | `input` | `append_event_raw` | runtime.lifecycle_observed `adb_target_recovery` | B | G10 | observation | D1c |
| 134 | `rh/host/input.rs:276` | `input` | `append_backend_open_failure_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | inside the critical action; D1c |
| 135 | `rh/host/input.rs:289` | `input` | `append_event_raw` | runtime.lifecycle_observed `adb_target_recovery` | B | G10 | observation | D1c |
| 136 | `rh/host/input.rs:472` | `application_control` | `append_scheduler_admitted_for_token` | scheduler.admitted | A | G4 | decision |  |
| 137 | `rh/host/input.rs:512` | `application_control` | `execute_critical` | application.intent, application.completed / application.failed | A | G6 | decision | DIRECT (`execute_critical`); listed in §4.1a (model `input.rs:510`); receipt sequence |
| 138 | `rh/host/input.rs:790` | `safe_reset` | `append_request_lifecycle` | cli.command / lab.request / ui.action, command.received, command.validated | A | G14 | decision | input reset request (`input.rs:731` reads `command.validated`) |
| 139 | `rh/host/input.rs:926` | `application_lifecycle` | `append_request_lifecycle` | cli.command / lab.request / ui.action, command.received, command.validated | A | G14 | decision | application lifecycle request |
| 140 | `rh/host/installation.rs:274` | `shutdown_under_gate` | `append_event` | runtime.lifecycle_observed `shutdown_request` | A | G16 | decision | read by the hot-install proof (`installation.rs:1044`, X2); receipt sequence |
| 141 | `rh/host/installation.rs:335` | `record_install_value` | `record_runtime_fact_with_event` | runtime.fact_recorded (`host.install_transition` / install pauses) | A | G3 | decision | Runtime scope; install G3 (receipt waits, §6.2) |
| 142 | `rh/host/installation.rs:428` | `install_refusal` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | install refusal (install stage) |
| 143 | `rh/host/installation.rs:655` | `tick_install_under_gate` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | install tick failure |
| 144 | `rh/host/installation.rs:711` | `initialize_installation` | `invalidate_runtime_fact` | runtime.fact_invalidated (install transition / pauses, `runtime_takeover`) | A | G3 | decision | Runtime scope; SU |
| 145 | `rh/host/installation.rs:894` | `fail_install_preparation` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | install preparation failure |
| 146 | `rh/host/instance_discovery.rs:114` | `instance_discovery_refused` | `append_event` | command.rejected | A | G14 | decision | refused request; receipt sequence |
| 147 | `rh/host/lab_operation.rs:33` | `run_contained_lab_operation` | `append_request_lifecycle` | cli.command / lab.request / ui.action, command.received, command.validated | A | G14 | decision | Lab `do` (device effect) |
| 148 | `rh/host/lab_operation.rs:518` | `run_contained_lab_operation` | `append_event` | command.validated / command.rejected | A | G14 | decision | Lab `do` terminal; receipt sequence |
| 149 | `rh/host/lab_operation.rs:782` | `evaluate_lab_operation_frame` | `append_event` | recognition.requested | B | G8 | observation |  |
| 150 | `rh/host/lab_operation.rs:857` | `evaluate_lab_operation_frame` | `append_event` | recognition.completed / recognition.failed | B | G8 | observation |  |
| 151 | `rh/host/lease.rs:219` | `queue_lease` | `append_lease_requested` | lease.requested | A | G4 | decision |  |
| 152 | `rh/host/lease.rs:282` | `queue_lease` | `append_lease_requested` | lease.requested | A | G4 | decision |  |
| 153 | `rh/host/lease.rs:283` | `queue_lease` | `append_scheduler_queued` | scheduler.queued | A | G4 | decision | receipt sequence |
| 154 | `rh/host/lease.rs:285` | `queue_lease` | `append_scheduler_preempted` | scheduler.preempted | A | G4 | decision | receipt sequence |
| 155 | `rh/host/lease.rs:608` | `append_scheduler_queued` | `append_scheduler_queued` (def) | scheduler.queued | A | — | decision | wrapper (def) |
| 156 | `rh/host/lease.rs:617` | `append_scheduler_queued` | `append_event` | scheduler.queued | A | G4 | decision |  |
| 157 | `rh/host/lease.rs:634` | `append_scheduler_preempted` | `append_scheduler_preempted` (def) | scheduler.preempted | A | — | decision | wrapper (def) |
| 158 | `rh/host/lease.rs:664` | `append_scheduler_preempted` | `append_event` | scheduler.preempted | A | G4 | decision |  |
| 159 | `rh/host/lease.rs:709` | `finish_queue_terminal` | `append_queue_terminal` | scheduler.denied (queue terminal) | A | G4 | decision | receipt sequence |
| 160 | `rh/host/lease.rs:748` | `cancel_instance_queue` | `append_queue_terminal` | scheduler.denied (queue terminal) | A | G4 | decision |  |
| 161 | `rh/host/lease.rs:840` | `append_queue_terminal` | `append_queue_terminal` (def) | scheduler.denied (queue terminal) | A | — | decision | wrapper (def) |
| 162 | `rh/host/lease.rs:855` | `append_queue_terminal` | `append_event` | scheduler.denied | A | G4 | decision |  |
| 163 | `rh/host/lease.rs:947` | `perform_transfer` | `append_event` | lease.transition_intent | A | G4 | decision |  |
| 164 | `rh/host/lease.rs:961` | `perform_transfer` | `append_event` | lease.transferred | A | G4 | decision | hand-over; receipt sequence |
| 165 | `rh/host/lease.rs:1186` | `acquire_lease_guarded` | `append_lease_requested` | lease.requested | A | G4 | decision |  |
| 166 | `rh/host/lease.rs:1228` | `require_performance_control_lease` | `append_lease_requested` | lease.requested | A | G4 | decision |  |
| 167 | `rh/host/lease.rs:1265` | `grant_prepared_lease` | `append_lease_requested` | lease.requested | A | G4 | decision |  |
| 168 | `rh/host/lease.rs:1266` | `grant_prepared_lease` | `append_scheduler_admitted` | scheduler.admitted | A | G4 | decision |  |
| 169 | `rh/host/lease.rs:1307` | `grant_prepared_lease_with_links` | `execute_critical` | lease.transition_intent, lease.granted / lease.transition_failed | A | G4 | decision | DIRECT (`execute_critical`); listed in §4.1a (model `ls:1304`) |
| 170 | `rh/host/lease.rs:1480` | `renew_lease` | `append_scheduler_admitted_for_token` | scheduler.admitted | A | G4 | decision |  |
| 171 | `rh/host/lease.rs:1504` | `renew_lease` | `execute_critical` | lease.transition_intent, lease.renewed / lease.transition_failed | A | G4 | decision | DIRECT (`execute_critical`); listed in §4.1a (model `ls:1501`) |
| 172 | `rh/host/lease.rs:1614` | `release_lease` | `append_scheduler_admitted_for_token` | scheduler.admitted | A | G4 | decision |  |
| 173 | `rh/host/lease.rs:1664` | `release_lease` | `execute_critical` | lease.transition_intent, lease.released / lease.transition_failed | A | G4 | decision | DIRECT (`execute_critical`); listed in §4.1a (model `ls:1661`) |
| 174 | `rh/host/lease.rs:1873` | `release_via_transfer` | `append_event` | lease.transition_intent | A | G4 | decision |  |
| 175 | `rh/host/lease.rs:1885` | `release_via_transfer` | `append_event` | lease.released | A | G4 | decision | receipt sequence |
| 176 | `rh/host/lease.rs:2057` | `hand_off_to_ladder` | `append_lease_requested` | lease.requested | A | G4 | decision | ladder hand-off |
| 177 | `rh/host/lease.rs:2059` | `hand_off_to_ladder` | `append_scheduler_queued_hand_off` | scheduler.queued | A | G4 | decision | ladder hand-off |
| 178 | `rh/host/lease.rs:2087` | `append_scheduler_queued_hand_off` | `append_scheduler_queued_hand_off` (def) | scheduler.queued | A | — | decision | wrapper (def) |
| 179 | `rh/host/lease.rs:2095` | `append_scheduler_queued_hand_off` | `append_event` | scheduler.queued | A | G4 | decision |  |
| 180 | `rh/host/lease.rs:2313` | `cleanup_token_guarded` | `execute_critical` | lease.transition_intent, lease.expired / lease.released / lease.transition_failed | A | G4 | decision | DIRECT (`execute_critical`); listed in §4.1a (model `ls:2134`) |
| 181 | `rh/host/lease.rs:2453` | `cleanup_via_transfer` | `append_event` | lease.transition_intent | A | G4 | decision |  |
| 182 | `rh/host/lease.rs:2466` | `cleanup_via_transfer` | `append_event` | lease.expired, lease.released | A | G4 | decision |  |
| 183 | `rh/host/lease.rs:2750` | `cleanup_connection` | `append_queue_terminal` | scheduler.denied (queue terminal) | A | G4 | decision |  |
| 184 | `rh/host/lease.rs:2896` | `require_bound_endpoint` | `append_event` | command.rejected | A | G14 | decision | refused request; receipt sequence |
| 185 | `rh/host/lease.rs:2936` | `append_lease_requested` | `append_lease_requested` (def) | lease.requested | A | — | decision | wrapper (def) |
| 186 | `rh/host/lease.rs:2944` | `append_lease_requested` | `append_event` | lease.requested | A | G4 | decision |  |
| 187 | `rh/host/lease.rs:2957` | `append_scheduler_admitted` | `append_scheduler_admitted` (def) | scheduler.admitted | A | — | decision | wrapper (def) |
| 188 | `rh/host/lease.rs:2963` | `append_scheduler_admitted` | `append_scheduler_admitted_for` | scheduler.admitted | A | — | decision | forward |
| 189 | `rh/host/lease.rs:2971` | `append_scheduler_admitted_for_token` | `append_scheduler_admitted_for_token` (def) | scheduler.admitted | A | — | decision | wrapper (def) |
| 190 | `rh/host/lease.rs:2977` | `append_scheduler_admitted_for_token` | `append_scheduler_admitted_for` | scheduler.admitted | A | — | decision | forward |
| 191 | `rh/host/lease.rs:2985` | `append_scheduler_admitted_for` | `append_scheduler_admitted_for` (def) | scheduler.admitted | A | — | decision | wrapper (def) |
| 192 | `rh/host/lease.rs:2995` | `append_scheduler_admitted_for` | `append_event` | scheduler.admitted | A | G4 | decision |  |
| 193 | `rh/host/lease.rs:3033` | `scheduler_denied_error` | `append_event_raw` | scheduler.denied | A | G4 | decision | receipt sequence |
| 194 | `rh/host/lease.rs:3270` | `pump` | `append_scheduler_admitted` | scheduler.admitted | A | G4 | decision | queue pump |
| 195 | `rh/host/lease.rs:3356` | `cancel_host_claim` | `append_queue_terminal` | scheduler.denied (queue terminal) | A | G4 | decision |  |
| 196 | `rh/host/lease.rs:3541` | `request_host_claim` | `append_lease_requested` | lease.requested | A | G4 | decision | host claim |
| 197 | `rh/host/lease.rs:3551` | `request_host_claim` | `append_lease_requested` | lease.requested | A | G4 | decision | host claim |
| 198 | `rh/host/lease.rs:3555` | `request_host_claim` | `append_lease_requested` | lease.requested | A | G4 | decision | host claim |
| 199 | `rh/host/lease.rs:3556` | `request_host_claim` | `append_scheduler_queued` | scheduler.queued | A | G4 | decision | host claim |
| 200 | `rh/host/lease.rs:3641` | `await_operator_claim` | `append_queue_terminal` | scheduler.denied (queue terminal) | A | G4 | decision | receipt sequence |
| 201 | `rh/host/lease.rs:3686` | `renew_held_lease` | `append_scheduler_admitted_for_token` | scheduler.admitted | A | G4 | decision |  |
| 202 | `rh/host/lease.rs:3710` | `renew_held_lease` | `execute_critical` | lease.transition_intent, lease.renewed / lease.transition_failed | A | G4 | decision | DIRECT (`execute_critical`); listed in §4.1a (model `ls:3315`) |
| 203 | `rh/host/lifecycle.rs:34` | `append_lifecycle_observed` | `append_lifecycle_observed` (def) | runtime.lifecycle_observed (phase from caller) | per phase | — | per phase | wrapper (def), model `lifecycle.rs:31` |
| 204 | `rh/host/lifecycle.rs:39` | `append_lifecycle_observed` | `append_event_raw` | runtime.lifecycle_observed (phase from caller) | per phase | — | per phase | forward to `f` |
| 205 | `rh/host/lifecycle.rs:51` | `append_stdio_close_observations` | `append_stdio_close_observations` (def) | runtime.lifecycle_observed `vendor_stdio_close` | B | — | decision | wrapper (def), model `lifecycle.rs:48` |
| 206 | `rh/host/lifecycle.rs:79` | `append_stdio_close_observations` | `append_event_under_fact_gate` | runtime.lifecycle_observed `vendor_stdio_close` | B | G10 | decision | UFG |
| 207 | `rh/host/lifecycle.rs:116` | `append_lifecycle_failure` | `append_lifecycle_failure` (def) | runtime.failed (lifecycle failure) | B | — | decision | wrapper (def), model `lifecycle.rs:113` |
| 208 | `rh/host/lifecycle.rs:123` | `append_lifecycle_failure` | `append_lifecycle_failure_with_severity` | runtime.failed (lifecycle failure) | B | — | decision | forward |
| 209 | `rh/host/lifecycle.rs:129` | `append_lifecycle_failure_with_severity` | `append_lifecycle_failure_with_severity` (def) | runtime.failed (lifecycle failure), lifecycle_observed `vendor_stdio_close` | B | G16 | decision | wrapper (def), model `lifecycle.rs:126`; the G16 failure choke point |
| 210 | `rh/host/lifecycle.rs:159` | `append_lifecycle_failure_with_severity` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | — | decision | recursion (complete failure, primary) |
| 211 | `rh/host/lifecycle.rs:179` | `append_lifecycle_failure_with_severity` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | — | decision | recursion (complete failure, secondary) |
| 212 | `rh/host/lifecycle.rs:218` | `append_lifecycle_failure_with_severity` | `append_stdio_close_observations` | runtime.lifecycle_observed `vendor_stdio_close` | B | G10 | decision | stdio residue of the failing error |
| 213 | `rh/host/lifecycle.rs:303` | `append_lifecycle_failure_with_severity` | `append_event_under_fact_gate` | runtime.failed (lifecycle failure) | B | G16 | decision | UFG |
| 214 | `rh/host/lifecycle.rs:463` | `record_lifecycle_result` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | `record_lifecycle_result` (stage from caller) |
| 215 | `rh/host/lifecycle.rs:529` | `record_required_failure_with_severity` | `append_lifecycle_failure_with_severity` | runtime.failed (lifecycle failure) | B | G16 | decision | `record_required_failure*` (stage from caller) |
| 216 | `rh/host/lifecycle.rs:538` | `append_connection_failure` | `append_connection_failure` (def) | runtime.failed | B | — | decision | wrapper (def) |
| 217 | `rh/host/lifecycle.rs:547` | `append_connection_failure` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | connection failure with a lifecycle stage |
| 218 | `rh/host/lifecycle.rs:559` | `append_connection_failure` | `append_event_raw` | runtime.failed | B | G14 | decision | IPC connection failure |
| 219 | `rh/host/lifecycle.rs:600` | `append_runtime_start_event` | `append_runtime_start_event` (def) | runtime.started / runtime.takeover | A | — | decision | wrapper (def) |
| 220 | `rh/host/lifecycle.rs:622` | `append_runtime_start_event` | `GlobalLedger::append` | runtime.started / runtime.takeover | A | G16 | decision | DIRECT; listed in §4.1a (model `lifecycle.rs:619`); SU |
| 221 | `rh/host/lifecycle.rs:654` | `append_instance_binding_events` | `append_instance_binding_events` (def) | runtime.instance_bound | A | — | decision | wrapper (def) |
| 222 | `rh/host/lifecycle.rs:675` | `append_instance_binding_events` | `GlobalLedger::append` | runtime.instance_bound | A | G16 | decision | DIRECT; listed in §4.1a (model `lifecycle.rs:672`); SU |
| 223 | `rh/host/material_read.rs:361` | `record_material_read_failure` | `append_lifecycle_failure_with_severity` | runtime.failed (lifecycle failure) | B | G16 | decision | material read failure (operation-level stage) |
| 224 | `rh/host/monitor_control.rs:36` | `configure_monitor` | `append_client_command_intent` | cli.command / lab.request / ui.action, command.received | A | G14 | decision | monitor configure |
| 225 | `rh/host/monitor_control.rs:74` | `clear_monitor` | `append_client_command_intent` | cli.command / lab.request / ui.action, command.received | A | G14 | decision | monitor clear |
| 226 | `rh/host/monitor_control.rs:114` | `monitor_mutation_success` | `append_event` | command.validated (runtime state) | A | G14 | decision | applied to the monitor registry; receipt sequence |
| 227 | `rh/host/monitor_control.rs:154` | `monitor_mutation_failure` | `append_event` | command.rejected | A | G14 | decision | receipt sequence |
| 228 | `rh/host/monitor_control.rs:196` | `run_monitor_probe` | `append_event_raw` | monitor.probe_requested | B | G8 | observation |  |
| 229 | `rh/host/monitor_control.rs:215` | `run_monitor_probe` | `append_event_raw` | monitor.probe_started | B | G8 | observation |  |
| 230 | `rh/host/monitor_control.rs:223` | `run_monitor_probe` | `append_event_raw` | capture.requested | B | G8 | observation | D1c |
| 231 | `rh/host/monitor_control.rs:231` | `run_monitor_probe` | `append_event_raw` | recognition.requested | B | G8 | observation |  |
| 232 | `rh/host/monitor_control.rs:262` | `run_monitor_probe` | `append_backend_open_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | D1c |
| 233 | `rh/host/monitor_control.rs:271` | `run_monitor_probe` | `append_backend_open_failure_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | D1c |
| 234 | `rh/host/monitor_control.rs:322` | `run_monitor_probe` | `append_event_raw` | capture.completed | B | G8 | observation | D1c |
| 235 | `rh/host/monitor_control.rs:413` | `run_monitor_probe` | `append_event_raw` | recognition.completed | B | G8 | observation |  |
| 236 | `rh/host/monitor_control.rs:435` | `run_monitor_probe` | `append_event_raw` | monitor.probe_completed | B | G8 | observation | applied to the monitor registry (`monitor.rs:393` keeps `last_sequence`) |
| 237 | `rh/host/monitor_control.rs:480` | `refuse_monitor_probe` | `append_event_raw` | monitor.probe_failed | B | G8 | observation | applied to the monitor registry; seq→projection (apply) |
| 238 | `rh/host/monitor_control.rs:534` | `record_monitor_recovery_coordination` | `append_event_raw` | monitor.recovery_admitted / monitor.recovery_deferred | B | G8 | decision |  |
| 239 | `rh/host/monitor_control.rs:617` | `finish_monitor_failure` | `append_event_raw` | capture.failed | B | G8 | observation | applied to the monitor registry; D1c (capture failure); seq→projection (apply) |
| 240 | `rh/host/monitor_control.rs:627` | `finish_monitor_failure` | `append_event_raw` | recognition.failed | B | G8 | observation |  |
| 241 | `rh/host/monitor_control.rs:643` | `finish_monitor_failure` | `append_event_raw` | monitor.probe_failed | B | G8 | observation | applied to the monitor registry; seq→projection (apply) |
| 242 | `rh/host/observation.rs:87` | `observe_readonly` | `append_request_lifecycle` | cli.command / lab.request / ui.action, command.received, command.validated | B | G8? | decision | read-only command audit (read-only observe); misfit M6 |
| 243 | `rh/host/observation.rs:100` | `observe_readonly` | `append_scheduler_admitted` | scheduler.admitted | A | G4 | decision |  |
| 244 | `rh/host/observation.rs:131` | `capture_sequence` | `append_request_lifecycle` | cli.command / lab.request / ui.action, command.received, command.validated | B | G8? | decision | read-only command audit (capture sequence); misfit M6 |
| 245 | `rh/host/observation.rs:144` | `capture_sequence` | `append_scheduler_admitted` | scheduler.admitted | A | G4 | decision |  |
| 246 | `rh/host/observation.rs:229` | `readonly_frame_admission_failure` | `append_event_raw` | capture.failed | B | G8 | observation | D1c (capture) |
| 247 | `rh/host/observation.rs:243` | `readonly_frame_admission_failure` | `append_event_raw` | recognition.failed | B | G8 | observation |  |
| 248 | `rh/host/observation.rs:278` | `capture_observation_with_links` | `append_event` | capture.requested | B | G8 | observation | D1c |
| 249 | `rh/host/observation.rs:286` | `capture_observation_with_links` | `append_event` | recognition.requested | B | G8 | observation |  |
| 250 | `rh/host/observation.rs:317` | `capture_observation_with_links` | `append_backend_open_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | D1c |
| 251 | `rh/host/observation.rs:327` | `capture_observation_with_links` | `append_backend_open_failure_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | D1c |
| 252 | `rh/host/observation.rs:359` | `capture_observation_with_links` | `append_event` | capture.failed | B | G8 | observation | D1c (capture) |
| 253 | `rh/host/observation.rs:368` | `capture_observation_with_links` | `append_event` | recognition.failed | B | G8 | observation | receipt sequence |
| 254 | `rh/host/observation.rs:495` | `capture_observation_with_links` | `append_capture_completed` | capture.completed | B | G8 | observation | D1c (capture) |
| 255 | `rh/host/observation.rs:501` | `capture_observation_with_links` | `append_event` | recognition.completed | B | G8 | observation |  |
| 256 | `rh/host/observation.rs:565` | `mark_observe_duplicate` | `append_event` | capture.dedup_window | B | G8 | observation | D1c |
| 257 | `rh/host/observation.rs:598` | `append_capture_completed` | `append_capture_completed` (def) | capture.completed | B | — | observation | wrapper (def); D1c |
| 258 | `rh/host/observation.rs:605` | `append_capture_completed` | `append_event` | capture.completed | B | G8 | observation | D1c (capture) |
| 259 | `rh/host/online_observation.rs:21` | `capture_material_failure` | `append_event_raw` | capture.failed | B | G8 | observation | D1c (capture) |
| 260 | `rh/host/online_observation.rs:68` | `record_frame_pressure_failures` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | frame pressure failure (operation-level stage) |
| 261 | `rh/host/online_observation.rs:87` | `observe_contained_page` | `append_request_lifecycle` | cli.command / lab.request / ui.action, command.received, command.validated | B | G8? | decision | read-only command audit (contained page observe); misfit M6 |
| 262 | `rh/host/online_observation.rs:188` | `observe_contained_page` | `append_scheduler_admitted` | scheduler.admitted | A | G4 | decision |  |
| 263 | `rh/host/online_observation.rs:197` | `observe_contained_page` | `append_event` | recognition.requested | B | G8 | observation |  |
| 264 | `rh/host/online_observation.rs:275` | `observe_contained_page` | `append_event` | recognition.completed / recognition.failed | B | G8 | observation | receipt sequence |
| 265 | `rh/host/online_observation.rs:369` | `observation_failure` | `append_event` | runtime.failed | B | G8 | decision | observation failure; receipt sequence |
| 266 | `rh/host/online_observation.rs:511` | `ObservationArtifactSink::append` | `ArtifactEventSink::append` (def) | artifact.created, artifact.verified (+ artifact.pin_recorded via frame_retention.rs:210) | B | — | decision | `ArtifactEventSink` impl (def); 9 construction sites (§7) |
| 267 | `rh/host/online_observation.rs:519` | `ObservationArtifactSink::append` | `GlobalLedger::append` | artifact.* | B | G9 | per type | DIRECT; listed in §4.1a |
| 268 | `rh/host/package_debug.rs:12` | `debug_package` | `append_event` | command.received | B | G14 | decision | Lab package debug request (in-memory debug run only) |
| 269 | `rh/host/package_debug.rs:24` | `debug_package` | `append_event` | command.rejected | B | G14 | decision | receipt sequence |
| 270 | `rh/host/package_debug.rs:75` | `debug_package` | `append_event` | command.rejected | B | G14 | decision | receipt sequence |
| 271 | `rh/host/package_debug.rs:114` | `debug_package` | `append_event` | task.requested | B | G7 | decision | debug run trace |
| 272 | `rh/host/package_debug.rs:125` | `debug_package` | `append_event` | task.started | B | G7 | decision | debug run trace |
| 273 | `rh/host/package_debug.rs:146` | `debug_package` | `append_event` | command.validated | B | G14 | decision | receipt sequence |
| 274 | `rh/host/performance.rs:108` | `sample_performance` | `reconcile_performance_control` | perf.* (control events) | B | G11 | per type | `reconcile_performance_control` → `record_performance_events` |
| 275 | `rh/host/performance.rs:110` | `sample_performance` | `reconcile_performance_tick` | perf.* (control events) | B | G11 | per type | `reconcile_performance_tick` → `record_performance_events` |
| 276 | `rh/host/performance.rs:134` | `require_business_capacity` | `append_event_raw` | scheduler.denied (capacity) | A | G4 | decision | receipt sequence |
| 277 | `rh/host/performance.rs:166` | `capacity_allows_transfer` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | capacity refusal of a transfer (operation-level stage) |
| 278 | `rh/host/performance.rs:176` | `reconcile_performance_control` | `reconcile_performance_control` (def) | perf.* (control events) | B | — | per type | host function (def), matches the guard's `reconcile_*` pattern; misfit M8 |
| 279 | `rh/host/performance.rs:191` | `reconcile_performance_tick` | `reconcile_performance_tick` (def) | perf.* (control events) | B | — | per type | host function (def), matches the guard's `reconcile_*` pattern; misfit M8 |
| 280 | `rh/host/performance.rs:267` | `record_performance_event` | `append_event_observed` | perf.* (`perf.balance_changed` decision; pressure, stutter, summary, monitor states observation) | B | G11 | per type | `record_performance_event`; actingd wakes on the Performance family (ResourcesChanged) |
| 281 | `rh/host/performance.rs:440` | `performance_monitor_loop` | `append_runtime_fact_snapshot_if_dirty` | runtime.fact_snapshot | B | T2 | decision | 60 s seal in `performance_monitor_loop` (model `hp:436-444`) |
| 282 | `rh/host/planning.rs:82` | `record_policy_planning_signal` | `GlobalLedger::append_transaction` | policy.planning_signal_observed + planning quota projection | A | G15 | decision | DIRECT (`append_transaction`); listed in §4.1a; `committed_signal` keeps the sequence |
| 283 | `rh/host/planning.rs:128` | `record_policy_planning_signal` | `append_agent_wake` | agent.wake_requested | A | G12 | decision | wake from a planning signal; trigger carries the source sequence |
| 284 | `rh/host/planning.rs:181` | `prepare_strategic_report_ipc` | `append_lifecycle_observed` | runtime.lifecycle_observed `strategic_report_entered` | B | stop? | decision | read-only planning IPC marker; misfit M7 |
| 285 | `rh/host/planning.rs:234` | `project_policy_forward_ipc` | `append_lifecycle_observed` | runtime.lifecycle_observed `policy_forward_entered` | B | stop? | decision | read-only planning IPC marker; misfit M7 |
| 286 | `rh/host/planning.rs:275` | `record_planning_result` | `append_lifecycle_observed` | runtime.lifecycle_observed (planning returned phase) | B | stop? | decision | read-only planning IPC marker; misfit M7 |
| 287 | `rh/host/planning.rs:276` | `record_planning_result` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | planning IPC failure |
| 288 | `rh/host/policy_catalog.rs:240` | `switch_policy_catalog` | `GlobalLedger::append` | catalog.transition_intent | A | G15 | decision | DIRECT; cited in G15 as `:232`, absent from §4.1a |
| 289 | `rh/host/policy_catalog.rs:263` | `switch_policy_catalog` | `GlobalLedger::append_transaction` | catalog.activated / catalog.rolled_back + catalog state | A | G15 | decision | DIRECT (`append_transaction`); listed in §4.1a |
| 290 | `rh/host/policy_catalog.rs:327` | `switch_policy_catalog` | `GlobalLedger::append` | catalog.transition_failed | A | G15 | decision | DIRECT; listed in §4.1a |
| 291 | `rh/host/policy_dispatch.rs:638` | `PolicyAdmissionAppender::append_durable` | `EventAppender::append_durable` (def) | policy.dispatch_intent (via `execute_critical`) | A | — | decision | `EventAppender` impl (def), model `pd:605` |
| 292 | `rh/host/policy_dispatch.rs:642` | `PolicyAdmissionAppender::append_durable` | `GlobalLedger::append_durable` | policy.dispatch_intent | A | G5 | decision | DIRECT (`append_durable`); listed in §4.1a |
| 293 | `rh/host/policy_dispatch.rs:689` | `evaluate_policy_cycle_authoritative` | `reconcile_pending_policy_settlements` | policy.execution_recorded, policy.dispatch_completed (continuation) | A | G5 | decision | settlement continuation via po:54 |
| 294 | `rh/host/policy_dispatch.rs:1511` | `admit_policy_dispatch` | `execute_critical` | policy.dispatch_intent, policy.dispatch_admitted / policy.dispatch_rejected | A | G5 | decision | DIRECT (`execute_critical` with `PolicyAdmissionAppender`); listed in §4.1a (model `pd:1471`); intent sequence kept by `PolicyHost` (`policy_host.rs:2441`) |
| 295 | `rh/host/policy_dispatch.rs:1894` | `record_policy_admission_result` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | policy admission failure (`PolicyRejected`, operation-level stage; M1) |
| 296 | `rh/host/policy_outcome.rs:15` | `reconcile_pending_policy_settlements` | `reconcile_pending_policy_settlements` (def) | policy.execution_recorded, policy.dispatch_completed (continuation) | A | — | decision | host function (def), matches the guard's `reconcile_*` pattern |
| 297 | `rh/host/policy_outcome.rs:32` | `reconcile_pending_policy_settlements` | `reconcile_scheduled_policy_outcomes_for` | policy.execution_recorded, policy.dispatch_completed (continuation) | A | G5 | decision | settlement continuation |
| 298 | `rh/host/policy_outcome.rs:54` | `reconcile_pending_policy_settlements` | `GlobalLedger::reconcile_scheduled_policy_settlement` | policy.execution_recorded, policy.dispatch_completed | A | G5 | decision | DIRECT; settlement continuation, model `po:52` → `g:1599` |
| 299 | `rh/host/policy_outcome.rs:295` | `recover_unreleased_policy_runs` | `append_event_raw` | lease.released | A | G4 | decision | SU; unreleased policy runs |
| 300 | `rh/host/policy_outcome.rs:307` | `recover_unreleased_policy_runs` | `append_event_raw` | runtime.failed | B | G5 | decision | SU; unreleased policy runs |
| 301 | `rh/host/policy_outcome.rs:492` | `record_scheduled_dispatch_diagnostic` | `append_event_raw` | runtime.failed | B | G5 | decision | scheduled dispatch diagnostic (SU and runtime) |
| 302 | `rh/host/policy_outcome.rs:1004` | `record_scheduled_policy_failure` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | scheduled policy failure |
| 303 | `rh/host/policy_outcome.rs:1011` | `record_scheduled_policy_failure` | `append_event_raw` | runtime.failed (resource declaration or diagnostic) | B | G5 | decision |  |
| 304 | `rh/host/policy_outcome.rs:1433` | `record_policy_dispatch_outcome_under_gate` | `append_event_raw` | policy.execution_recorded | A | G5 | decision | under the policy outcome gate |
| 305 | `rh/host/policy_outcome.rs:1523` | `finish_policy_dispatch_outcome` | `append_event_raw` | policy.dispatch_completed | A | G5 | decision |  |
| 306 | `rh/host/policy_outcome.rs:1678` | `reconcile_policy_dispatches` | `reconcile_policy_dispatches` (def) | policy.execution_recorded, policy.dispatch_completed (continuation), policy.dispatch_rejected | A | — | decision | host function (def), matches the guard's `reconcile_*` pattern |
| 307 | `rh/host/policy_outcome.rs:1684` | `reconcile_policy_dispatches` | `reconcile_scheduled_policy_outcomes` | policy.execution_recorded, policy.dispatch_completed (continuation) | A | G5 | decision | SU |
| 308 | `rh/host/policy_outcome.rs:1687` | `reconcile_policy_dispatches` | `GlobalLedger::reconcile_scheduled_policy_settlement` | policy.execution_recorded, policy.dispatch_completed | A | G5 | decision | DIRECT; SU; settlement continuation, model `po:1203` |
| 309 | `rh/host/policy_outcome.rs:1757` | `reconcile_policy_dispatches` | `GlobalLedger::append` | policy.dispatch_rejected | A | G5 | decision | DIRECT; SU; not in §4.1a |
| 310 | `rh/host/policy_outcome.rs:1763` | `reconcile_scheduled_policy_outcomes` | `reconcile_scheduled_policy_outcomes` (def) | policy.execution_recorded, policy.dispatch_completed (continuation) | A | — | decision | host function (def) |
| 311 | `rh/host/policy_outcome.rs:1769` | `reconcile_scheduled_policy_outcomes` | `reconcile_scheduled_policy_outcomes_for` | policy.execution_recorded, policy.dispatch_completed (continuation) | A | G5 | decision | forward |
| 312 | `rh/host/policy_outcome.rs:1773` | `reconcile_scheduled_policy_outcomes_for` | `reconcile_scheduled_policy_outcomes_for` (def) | policy.execution_recorded, policy.dispatch_completed (continuation) | A | — | decision | host function (def) |
| 313 | `rh/host/policy_outcome.rs:2038` | `reconcile_scheduled_policy_outcomes_for` | `GlobalLedger::reconcile_scheduled_policy_settlement` | policy.execution_recorded, policy.dispatch_completed | A | G5 | decision | DIRECT; settlement continuation, model `po:1495` |
| 314 | `rh/host/read_events.rs:363` | `observe_runtime_state` | `append_event` | command.validated (runtime state, status query) | B | stop | decision | §6.1: status query stops writing; the reply's source `{event_id, sequence}` uses the result (`read_events.rs:392`) |
| 315 | `rh/host/recovery_ladder.rs:280` | `record_preparation_finished` | `append_event_raw` | runtime.lifecycle_observed `instance_preparation_finished` | B | G4? | decision | ladder preparation stage record; misfit M5; seq→receipt |
| 316 | `rh/host/recovery_ladder.rs:437` | `recovery_admitted` | `append_recovery_ladder_fact` | runtime.lifecycle_observed `recovery_ladder_suppressed` | B | G4? | decision | misfit M5 |
| 317 | `rh/host/recovery_ladder.rs:568` | `admit_recovery_ladder` | `append_recovery_ladder_fact` | runtime.lifecycle_observed `recovery_ladder_suppressed` | B | G4? | decision | misfit M5 |
| 318 | `rh/host/recovery_ladder.rs:615` | `enqueue_ladder_claim` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | ladder claim enqueue failure |
| 319 | `rh/host/recovery_ladder.rs:649` | `climb_recovery_ladder` | `append_recovery_ladder_fact` | runtime.lifecycle_observed `recovery_ladder_suppressed` | B | G4? | decision | misfit M5 |
| 320 | `rh/host/recovery_ladder.rs:695` | `climb_recovery_ladder` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | ladder climb failure |
| 321 | `rh/host/recovery_ladder.rs:734` | `climb_recovery_ladder` | `append_recovery_ladder_fact` | runtime.lifecycle_observed `recovery_ladder_started` | B | G4? | decision | misfit M5 |
| 322 | `rh/host/recovery_ladder.rs:753` | `climb_recovery_ladder` | `append_recovery_rung_finished` | runtime.lifecycle_observed `recovery_rung_finished` | B | G4? | decision | misfit M5 |
| 323 | `rh/host/recovery_ladder.rs:781` | `climb_recovery_ladder` | `append_recovery_rung_finished` | runtime.lifecycle_observed `recovery_rung_finished` | B | G4? | decision | misfit M5 |
| 324 | `rh/host/recovery_ladder.rs:789` | `climb_recovery_ladder` | `append_recovery_ladder_fact` | runtime.lifecycle_observed `recovery_ladder_finished` | B | G4? | decision | misfit M5 |
| 325 | `rh/host/recovery_ladder.rs:799` | `climb_recovery_ladder` | `append_recovery_rung_finished` | runtime.lifecycle_observed `recovery_rung_finished` | B | G4? | decision | misfit M5 |
| 326 | `rh/host/recovery_ladder.rs:807` | `climb_recovery_ladder` | `append_recovery_ladder_fact` | runtime.lifecycle_observed `recovery_ladder_finished` | B | G4? | decision | misfit M5 |
| 327 | `rh/host/recovery_ladder.rs:818` | `climb_recovery_ladder` | `append_recovery_rung_finished` | runtime.lifecycle_observed `recovery_rung_finished` | B | G4? | decision | misfit M5 |
| 328 | `rh/host/recovery_ladder.rs:829` | `climb_recovery_ladder` | `append_recovery_rung_finished` | runtime.lifecycle_observed `recovery_rung_finished` | B | G4? | decision | misfit M5 |
| 329 | `rh/host/recovery_ladder.rs:846` | `climb_recovery_ladder` | `append_recovery_ladder_fact` | runtime.lifecycle_observed `recovery_ladder_finished` | B | G4? | decision | misfit M5 |
| 330 | `rh/host/recovery_ladder.rs:893` | `finish_interrupted_ladder` | `append_recovery_rung_finished` | runtime.lifecycle_observed `recovery_rung_finished` | B | G4? | decision | misfit M5 |
| 331 | `rh/host/recovery_ladder.rs:902` | `finish_interrupted_ladder` | `append_recovery_ladder_fact` | runtime.lifecycle_observed `recovery_ladder_finished` | B | G4? | decision | misfit M5 |
| 332 | `rh/host/recovery_ladder.rs:1135` | `stop_assigned_application` | `append_event_raw` | command.received, command.validated (ladder application stop) | A | G14 | decision | synthetic request record of the ladder's application stop |
| 333 | `rh/host/recovery_ladder.rs:1153` | `stop_assigned_application` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision |  |
| 334 | `rh/host/recovery_ladder.rs:1204` | `recovery_emulator_restart` | `append_event_raw` | command.received | A | G13 | decision | ladder emulator restart intent |
| 335 | `rh/host/recovery_ladder.rs:1257` | `recovery_emulator_restart` | `append_recovery_ladder_fact` | runtime.lifecycle_observed `recovery_instance_stopped` | B | G13 | decision |  |
| 336 | `rh/host/recovery_ladder.rs:1288` | `recovery_emulator_restart` | `append_event_raw` | runtime.lifecycle_observed `recovery_environment_ready` | B | G13 | decision | receipt sequence |
| 337 | `rh/host/recovery_ladder.rs:1603` | `append_recovery_rung_finished` | `append_recovery_rung_finished` (def) | runtime.lifecycle_observed `recovery_rung_finished` | B | — | decision | wrapper (def) |
| 338 | `rh/host/recovery_ladder.rs:1612` | `append_recovery_rung_finished` | `append_recovery_ladder_fact` | runtime.lifecycle_observed `recovery_rung_finished` | B | G4? | decision | forward |
| 339 | `rh/host/recovery_ladder.rs:1629` | `append_recovery_ladder_fact` | `append_recovery_ladder_fact` (def) | runtime.lifecycle_observed (ladder phases) | B | — | decision | wrapper (def) |
| 340 | `rh/host/recovery_ladder.rs:1635` | `append_recovery_ladder_fact` | `append_event_raw` | runtime.lifecycle_observed (ladder phases) | B | G4? | decision | forward to `f`; misfit M5 |
| 341 | `rh/host/requests.rs:117` | `process_request_observed` | `append_event_raw` | runtime.failed (resource declaration) | B | G14 | decision | request-boundary rejection; receipt sequence |
| 342 | `rh/host/requests.rs:891` | `connection_boundary` | `append_connection_failure` | runtime.failed | B | G14 | decision | IPC connection boundary |
| 343 | `rh/host/requests.rs:1168` | `append_request_lifecycle` | `append_request_lifecycle` (def) | cli.command / lab.request / ui.action, command.received, command.validated | per caller | — | decision | wrapper (def); class/gate per caller (write → A G14; read-only → B) |
| 344 | `rh/host/requests.rs:1177` | `append_request_lifecycle` | `append_client_command_intent` | cli.command / lab.request / ui.action, command.received | per caller | — | decision | forward |
| 345 | `rh/host/requests.rs:1178` | `append_request_lifecycle` | `append_event` | command.validated | per caller | G14 | decision |  |
| 346 | `rh/host/requests.rs:1193` | `append_scheduled_request_lifecycle` | `append_scheduled_request_lifecycle` (def) | command.received, command.validated | A | — | decision | wrapper (def) |
| 347 | `rh/host/requests.rs:1212` | `append_scheduled_request_lifecycle` | `append_request_lifecycle` | cli.command / lab.request / ui.action, command.received, command.validated | A | — | decision | forward (fixture provenance) |
| 348 | `rh/host/requests.rs:1225` | `append_scheduled_request_lifecycle` | `append_event` | command.received | A | G14 | decision |  |
| 349 | `rh/host/requests.rs:1233` | `append_scheduled_request_lifecycle` | `append_event` | command.validated | A | G14 | decision |  |
| 350 | `rh/host/requests.rs:1248` | `append_client_command_intent` | `append_client_command_intent` (def) | cli.command / lab.request / ui.action, command.received | per caller | — | decision | wrapper (def) |
| 351 | `rh/host/requests.rs:1295` | `append_client_command_intent` | `append_event` | cli.command / lab.request / ui.action | per caller | G14 | decision | client-side record of the request |
| 352 | `rh/host/requests.rs:1303` | `append_client_command_intent` | `append_event` | command.received | per caller | G14 | decision |  |
| 353 | `rh/host/resource_close.rs:230` | `retain_unconfirmed_resources` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | session close (operation-level stage) |
| 354 | `rh/host/resource_close.rs:345` | `retire_instance_resources` | `append_stdio_close_observations` | runtime.lifecycle_observed `vendor_stdio_close` | B | G10 | decision |  |
| 355 | `rh/host/resource_close.rs:352` | `retire_instance_resources` | `append_lifecycle_observed` | runtime.lifecycle_observed `resource_quiescence` | B | G10 | decision | device session close observation |
| 356 | `rh/host/resource_close.rs:369` | `retire_instance_resources` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | session close (operation-level stage) |
| 357 | `rh/host/resource_close.rs:540` | `finish_input_failure` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | session close after input failure |
| 358 | `rh/host/resource_close.rs:547` | `finish_input_failure` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision |  |
| 359 | `rh/host/resource_close.rs:583` | `finish_capture_failure_while_guarded` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | session close after capture failure |
| 360 | `rh/host/resource_close.rs:590` | `finish_capture_failure_while_guarded` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision |  |
| 361 | `rh/host/resource_close.rs:987` | `prepare_instance_connection_with_cooldown` | `append_backend_open_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | connection preparation; D1c |
| 362 | `rh/host/resource_close.rs:996` | `prepare_instance_connection_with_cooldown` | `append_backend_open_failure_observations` | runtime.lifecycle_observed `backend_open_observed` | B | G10 | observation | connection preparation; D1c |
| 363 | `rh/host/resource_close.rs:1004` | `prepare_instance_connection_with_cooldown` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | connection preparation (carries the backend open codes counted by D1c) |
| 364 | `rh/host/resource_close.rs:1086` | `record_connection_preparation_failure` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | connection preparation failure |
| 365 | `rh/host/runtime_config.rs` (was `runtime_facts.rs:219` `record_config_manifest`) | `record_config_fact` | `record_runtime_fact` | runtime.fact_recorded (configuration inventory: `config.subsystems`, `config.selection`, `config.parameters.<section>`, `host.config_overrides`; `config.catalog`, Workflow #389 MCP-9a) | A | G3 | decision | Runtime scope; SU; Workflow #387 D1b: at start and at each `ConfigureSettings` (command record → G14); the start's restore notes are `runtime.failed` through `append_event_raw`; `config.catalog` at start, after a committed catalog transition and after an approval decision, each time only when it changed (`record_config_catalog`) |
| 366 | `rh/host/runtime_facts.rs:300` | `record_backend_selfcheck_facts` | `record_runtime_fact_under_gate` | runtime.fact_recorded (backend self-check values) | A | G3 | observation | instance scope; model §6.1 "self-check facts → gate" |
| 367 | `rh/host/runtime_facts.rs:314` | `record_backend_selfcheck_facts` | `append_event_under_fact_gate` | runtime.lifecycle_observed `device.self_check` | B | G10 | observation | UFG; D1c (self-check) |
| 368 | `rh/host/runtime_facts.rs:369` | `invalidate_backend_selfcheck_facts` | `invalidate_runtime_fact` | runtime.fact_invalidated (self-check values, `device_closed`) | A | G3 | observation | instance scope |
| 369 | `rh/host/runtime_facts.rs:586` | `record_runtime_fact` | `record_runtime_fact` (def) | runtime.fact_recorded | A | — | per scope | wrapper (def), model `rf:586` |
| 370 | `rh/host/runtime_facts.rs:590` | `record_runtime_fact` | `record_runtime_fact_with_event` | runtime.fact_recorded | A | — | per scope | forward |
| 371 | `rh/host/runtime_facts.rs:594` | `record_runtime_fact_with_event` | `record_runtime_fact_with_event` (def) | runtime.fact_recorded | A | — | per scope | wrapper (def) |
| 372 | `rh/host/runtime_facts.rs:600` | `record_runtime_fact_with_event` | `record_runtime_fact_and_event_under_gate` | runtime.fact_recorded | A | — | per scope | forward |
| 373 | `rh/host/runtime_facts.rs:612` | `record_runtime_fact_under_gate` | `record_runtime_fact_under_gate` (def) | runtime.fact_recorded | A | — | per scope | wrapper (def) |
| 374 | `rh/host/runtime_facts.rs:616` | `record_runtime_fact_under_gate` | `record_runtime_fact_and_event_under_gate` | runtime.fact_recorded | A | — | per scope | forward |
| 375 | `rh/host/runtime_facts.rs:620` | `record_runtime_fact_and_event_under_gate` | `record_runtime_fact_and_event_under_gate` (def) | runtime.fact_recorded | A | G3 | per scope | wrapper (def), G3 entry `rf:586-646` |
| 376 | `rh/host/runtime_facts.rs:632` | `record_runtime_fact_and_event_under_gate` | `append_event_under_fact_gate` | runtime.fact_recorded | A | G3 | per scope | UFG |
| 377 | `rh/host/runtime_facts.rs:666` | `record_changed_instance_string_fact` | `record_runtime_fact` | runtime.fact_recorded (task identity strings) | A | G3 | observation | instance scope |
| 378 | `rh/host/runtime_facts.rs:784` | `record_settlement_fact_records` | `record_runtime_fact` | runtime.fact_recorded (task settlement values) | A | G3 | observation | instance scope; written from policy settlement |
| 379 | `rh/host/runtime_facts.rs:799` | `invalidate_runtime_fact` | `invalidate_runtime_fact` (def) | runtime.fact_invalidated | A | G3 | per scope | wrapper (def), G3 entry `rf:799-853` |
| 380 | `rh/host/runtime_facts.rs:831` | `invalidate_runtime_fact` | `append_event_under_fact_gate` | runtime.fact_invalidated | A | G3 | per scope | UFG |
| 381 | `rh/host/runtime_facts.rs:871` | `append_runtime_fact_snapshot_if_dirty` | `append_runtime_fact_snapshot_if_dirty` (def) | runtime.fact_snapshot | B | — | decision | wrapper (def) |
| 382 | `rh/host/runtime_facts.rs:898` | `append_runtime_fact_snapshot_if_dirty` | `append_event_under_fact_gate` | runtime.fact_snapshot | B | T2 | decision | UFG; §6.1: becomes the ledger's T2 self-seal |
| 383 | `rh/host/runtime_facts.rs:1123` | `recover_runtime_fact_store` | `GlobalLedger::append` | runtime.lifecycle_observed `fact_snapshot_set_skipped` | B | G16 | decision | DIRECT; SU (workbench recovery); not in §4.1a |
| 384 | `rh/host/runtime_facts.rs:1183` | `recover_runtime_fact_store` | `append_takeover_invalidations` | runtime.fact_invalidated (`runtime_takeover`) | A | G3 | observation | SU; instance scope |
| 385 | `rh/host/runtime_facts.rs:1193` | `append_takeover_invalidations` | `append_takeover_invalidations` (def) | runtime.fact_invalidated (`runtime_takeover`) | A | — | observation | wrapper (def) |
| 386 | `rh/host/runtime_facts.rs:1239` | `append_takeover_invalidations` | `GlobalLedger::append` | runtime.fact_invalidated (`runtime_takeover`) | A | G3 | observation | DIRECT; SU; not in §4.1a |
| 387 | `rh/host/saved_artifact_ocr.rs:25` | `recognize_artifact` | `append_event` | recognition.requested | B | G8 | observation |  |
| 388 | `rh/host/saved_artifact_ocr.rs:45` | `recognize_artifact` | `append_event` | recognition.failed | B | G8 | observation | receipt sequence |
| 389 | `rh/host/saved_artifact_ocr.rs:398` | `recognize_artifact_inner` | `append_event_raw` | recognition.completed | B | G8 | observation | receipt + position check use the sequence (`saved_artifact_ocr.rs:437`) |
| 390 | `rh/host/scheduling_pause.rs:129` | `record_scheduling_pauses` | `record_runtime_fact_with_event` | runtime.fact_recorded (`host.scheduling_pause`) | A | G3 | decision | Runtime scope; §6.1: pause/resume workbench change → G3 (command record → G14) |
| 391 | `rh/host/signatures.rs:35` | `register_signature` | `append_event` | signature.registered | A | G14 | decision | signature catalog is re-read from the ledger (`signatures.rs:15-23`); misfit M9; seq→receipt |
| 392 | `rh/host/signatures.rs:67` | `retire_signature` | `append_event` | signature.retired | A | G14 | decision | misfit M9; seq→receipt |
| 393 | `rh/host/signatures.rs:111` | `match_signatures` | `append_event` | signature.matched | B | G14 | decision | audit of a match request; seq→receipt |
| 394 | `rh/host/startup_package.rs:298` | `prepare_startup_package` | `append_event` | runtime.lifecycle_observed `startup_package_scheduled` | B | G13 | decision | after emulator start |
| 395 | `rh/host/startup_package.rs:343` | `record_startup_package_failure` | `append_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision |  |
| 396 | `rh/host/startup_package.rs:350` | `record_startup_package_failure` | `append_event_raw` | runtime.failed (resource declaration or diagnostic) | B | G13 | decision |  |
| 397 | `rh/host/state_control.rs:46` | `stage_release_set` | `GlobalLedger::append_transaction` | release.staged + release state | A | G15 | decision | DIRECT (`append_transaction`); listed in §4.1a |
| 398 | `rh/host/state_control.rs:119` | `switch_release_set` | `GlobalLedger::append` | release.transition_intent | A | G15 | decision | DIRECT; cited in G15 as `:113`, absent from §4.1a |
| 399 | `rh/host/state_control.rs:140` | `switch_release_set` | `GlobalLedger::append_transaction` | release.activated / release.rolled_back + release state | A | G15 | decision | DIRECT (`append_transaction`); listed in §4.1a |
| 400 | `rh/host/state_control.rs:168` | `switch_release_set` | `GlobalLedger::append` | release.transition_failed | A | G15 | decision | DIRECT; listed in §4.1a |
| 401 | `rh/host/state_control.rs:228` | `reconcile_runtime_state` | `reconcile_runtime_state` (def) | state.migrated, release.staged | A | — | decision | host function (def), matches the guard's `reconcile_*` pattern |
| 402 | `rh/host/state_control.rs:271` | `reconcile_runtime_state` | `append_runtime_state_event` | state.migrated | A | G15 | decision | SU |
| 403 | `rh/host/state_control.rs:279` | `reconcile_runtime_state` | `reconcile_release_state` | state.migrated, release.staged | A | G15 | decision | SU |
| 404 | `rh/host/state_control.rs:282` | `append_runtime_state_event` | `append_runtime_state_event` (def) | state.migrated | A | — | decision | wrapper (def) |
| 405 | `rh/host/state_control.rs:297` | `append_runtime_state_event` | `GlobalLedger::append` | state.migrated | A | G15 | decision | DIRECT; SU; not in §4.1a |
| 406 | `rh/host/state_control.rs:301` | `reconcile_release_state` | `reconcile_release_state` (def) | state.migrated, release.staged | A | — | decision | host function (def) |
| 407 | `rh/host/state_control.rs:324` | `reconcile_release_state` | `GlobalLedger::append_transaction` | state.migrated (release baseline) + state document | A | G15 | decision | DIRECT (`append_transaction`); SU; listed in §4.1a |
| 408 | `rh/host/state_control.rs:370` | `reconcile_release_state` | `GlobalLedger::append_transaction` | release.staged + state document | A | G15 | decision | DIRECT (`append_transaction`); SU; listed in §4.1a |
| 409 | `rh/ledger_maintenance.rs:311` | `run_locked` | `LedgerMaintenance::import` | ledger.recovered (`storage_cutover`, migration record) | W | W2 | decision | DIRECT (`LedgerMaintenance::import`, `migration.rs:271`); draft built at `:303`; offline `actingd ledger-maintenance import`; `dry-run` writes nothing |
| 410 | `rh/monitor.rs:88` | `MonitorRegistry::open` | `GlobalLedger::append` | runtime.failed (Fatal, monitor registry restore) | B | G16 | decision | DIRECT; SU; not in §4.1a |
| 411 | `rh/monitor.rs:174` | `MonitorRegistry::recover` | `GlobalLedger::append` | command.validated (runtime state `monitor_imported`) | A | G16 | decision | DIRECT; SU; applied to the monitor registry; not in §4.1a; misfit M9; seq→projection (apply) |
| 412 | `rh/owner_unlock.rs:48` | `run` | `owner_unlock::record` | cli.command (owner unlock) | W | W1 | decision | offline `actingd unlock-owner`; calls `record` |
| 413 | `rh/owner_unlock.rs:60` | `record` | `owner_unlock::record` (def) | cli.command (owner unlock) | W | — | decision | wrapper (def); opens the writer at `:72-88` |
| 414 | `rh/owner_unlock.rs:108` | `record` | `GlobalLedger::append` | cli.command (owner unlock; payload variant `OwnerUnlock`) | W | W1 | decision | DIRECT; the model's `client.owner_unlock` |
| 415 | `rh/performance/capacity.rs:442` | `PerformanceMonitor::sample_and_record_capacity` | `capacity::append` | perf.summary | B | G11 | observation | capacity summary; sequence kept as `CapacityFactReference` (`capacity.rs:208`) |
| 416 | `rh/performance/capacity.rs:496` | `PerformanceMonitor::degrade_for_rejected_summary` | `capacity::append` | perf.monitor_degraded (for a rejected summary) | B | G11 | observation |  |
| 417 | `rh/performance/capacity.rs:569` | `CapacityMonitor::record_transitions` | `capacity::append` | perf.monitor_degraded | B | G11 | observation |  |
| 418 | `rh/performance/capacity.rs:593` | `CapacityMonitor::record_transitions` | `capacity::append` | perf.monitor_recovered | B | G11 | observation |  |
| 419 | `rh/performance/capacity.rs:646` | `CapacityMonitor::record_transitions` | `capacity::append` | perf.pressure_started | B | G11 | observation |  |
| 420 | `rh/performance/capacity.rs:667` | `CapacityMonitor::record_transitions` | `capacity::append` | perf.pressure_ended | B | G11 | observation |  |
| 421 | `rh/performance/capacity.rs:684` | `append` | `capacity::append` (def) | perf.* | B | — | observation | free function (def) |
| 422 | `rh/performance/capacity.rs:698` | `append` | `GlobalLedger::append` | perf.* | B | G11 | observation | DIRECT; listed in §4.1a |
| 423 | `rh/policy_host/catalog_transaction.rs:268` | `CatalogStore::migrate_legacy_active_pointer` | `GlobalLedger::append_transaction` | state.migrated + catalog pointer migration | A | G15 | decision | DIRECT (`append_transaction`); SU (policy host open); listed in §4.1a |
| 424 | `rh/provider_startup.rs:22` | `ProviderStartup::record` | `ProviderStartup::record` (def) | provider.startup_observed | B | — | observation | `ProviderStartup::record` (def), handed to actingd's assembly |
| 425 | `rh/provider_startup.rs:48` | `ProviderStartup::record` | `GlobalLedger::append` | provider.startup_observed | B | G10? | observation | DIRECT; SU; not in §4.1a; misfit M2 |
| 426 | `rh/provider_startup.rs:59` | `ProviderStartup::failed` | `ProviderStartup::failed` (def) | provider.startup_observed (Failed, Fatal) | B | — | observation | `ProviderStartup::failed` (def) |
| 427 | `rh/provider_startup.rs:66` | `ProviderStartup::failed` | `ProviderStartup::record` | provider.startup_observed (Failed, Fatal) | B | G10? | observation | forward to `record` |
| 428 | `ad/config/provider_startup.rs:46` | `ConfiguredProvider::assemble_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 429 | `ad/config/provider_startup.rs:62` | `ConfiguredProvider::assemble_provider` | `ProviderStartup::failed` | provider.startup_observed (Failed, Fatal) | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 430 | `ad/config/provider_startup.rs:75` | `ConfiguredProvider::assemble_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 431 | `ad/config/provider_startup.rs:91` | `bind_discovered_instances` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 432 | `ad/config/provider_startup.rs:109` | `bind_discovered_instances` | `ProviderStartup::failed` | provider.startup_observed (Failed, Fatal) | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 433 | `ad/config/provider_startup.rs:143` | `bind_discovered_instances` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 434 | `ad/config/provider_startup.rs:164` | `bind_discovered_instances` | `ProviderStartup::failed` | provider.startup_observed (Failed, Fatal) | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 435 | `ad/config/provider_startup.rs:166` | `bind_discovered_instances` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 436 | `ad/config/provider_startup.rs:180` | `admit_capability_profile` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 437 | `ad/config/provider_startup.rs:190` | `admit_capability_profile` | `ProviderStartup::failed` | provider.startup_observed (Failed, Fatal) | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 438 | `ad/config/provider_startup.rs:212` | `admit_capability_profile` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 439 | `ad/config/provider_startup.rs:223` | `admit_capability_profile` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 440 | `ad/config/provider_startup.rs:441` | `assemble_vision_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 441 | `ad/config/provider_startup.rs:443` | `assemble_vision_provider` | `ProviderStartup::failed` | provider.startup_observed (Failed, Fatal) | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 442 | `ad/config/provider_startup.rs:485` | `assemble_vision_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 443 | `ad/config/provider_startup.rs:509` | `assemble_vision_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 444 | `ad/config/provider_startup.rs:524` | `assemble_vision_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 445 | `ad/config/provider_startup.rs:535` | `assemble_vision_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 446 | `ad/config/provider_startup.rs:538` | `assemble_vision_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 447 | `ad/config/provider_startup.rs:540` | `assemble_vision_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 448 | `ad/config/provider_startup.rs:542` | `assemble_vision_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 449 | `ad/config/provider_startup.rs:548` | `assemble_vision_provider` | `ProviderStartup::failed` | provider.startup_observed (Failed, Fatal) | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 450 | `ad/config/provider_startup.rs:560` | `assemble_vision_provider` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 451 | `ad/config/provider_startup.rs:571` | `text` | `ProviderStartup::failed` | provider.startup_observed (Failed, Fatal) | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 452 | `ad/config/provider_startup.rs:596` | `binding` | `ProviderStartup::record` | provider.startup_observed | B | G10? | observation | SU; actingd provider assembly; misfit M2 |
| 453 | `ad/main.rs:176` | `run` | `ActingdError::record_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | actingd → `RuntimeHost::record_lifecycle_failure` (rh/host.rs:3289) |
| 454 | `ad/main.rs:202` | `run` | `ActingdError::record_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | actingd → `RuntimeHost::record_lifecycle_failure` (rh/host.rs:3289) |
| 455 | `ad/main.rs:541` | `monitor` | `ActingdError::record_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | actingd → `RuntimeHost::record_lifecycle_failure` (rh/host.rs:3289) |
| 456 | `ad/main.rs:609` | `monitor_policy` | `ActingdError::record_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | actingd → `RuntimeHost::record_lifecycle_failure` (rh/host.rs:3289) |
| 457 | `ad/main.rs:766` | `monitor_policy` | `ActingdError::record_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | actingd → `RuntimeHost::record_lifecycle_failure` (rh/host.rs:3289) |
| 458 | `ad/main.rs:938` | `finish_policy_closeout` | `ActingdError::record_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | actingd → `RuntimeHost::record_lifecycle_failure` (rh/host.rs:3289) |
| 459 | `ad/main.rs:954` | `close_policy_host` | `ActingdError::record_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | actingd → `RuntimeHost::record_lifecycle_failure` (rh/host.rs:3289) |
| 460 | `ad/main.rs:1785` | `ActingdError::record_lifecycle_failure` | `record_lifecycle_failure` (def) | runtime.failed (lifecycle failure) | B | — | decision | actingd wrapper (def) over the host's public API |
| 461 | `ad/main.rs:1809` | `ActingdError::record_lifecycle_failure` | `RuntimeHost::record_lifecycle_failure` | runtime.failed (lifecycle failure) | B | G16 | decision | actingd → `RuntimeHost::record_lifecycle_failure` (rh/host.rs:3289) |

### A.3 Totals and comparison with the model's figures

- Rows: **461** = **398** call sites + **63** wrapper / impl definitions, in 48 files (2 in actingd).
- By class (all rows / call sites only): A 180 / 152, B 256 / 231, W 4 / 3, per caller 19 / 11, per phase 2 / 1.
- By gate (all rows / call sites only): G1 5 / 3, G2 2 / 2, G3 19 / 17, G4 51 / 51, G4? 17 / 17, G5 17 / 17, G6 2 / 2, G7 38 / 38, G8 36 / 36, G8? 3 / 3, G9 5 / 5, G10 19 / 19, G10? 27 / 27, G11 10 / 10, G12 8 / 8, G13 12 / 12, G14 37 / 37, G15 16 / 16, G16 52 / 51, T2 2 / 2, W1 2 / 2, W2 1 / 1, stop 1 / 1, stop? 3 / 3, ledger-own? 3 / 3, — 73 / 15.
- Direct ledger calls (DIRECT, call sites): **48** — `rh/host.rs:1260`, `rh/host/agent_control.rs:324`, `rh/host/client_events.rs:21`, `rh/host/contained_task.rs:4093`, `rh/host/device_diagnostic.rs:205`, `rh/host/facts.rs:612`, `rh/host/facts.rs:616`, `rh/host/facts.rs:666`, `rh/host/frame_retention.rs:210`, `rh/host/frame_retention.rs:702`, `rh/host/frame_retention.rs:709`, `rh/host/governance.rs:97`, `rh/host/input.rs:199`, `rh/host/input.rs:512`, `rh/host/lease.rs:1307`, `rh/host/lease.rs:1504`, `rh/host/lease.rs:1664`, `rh/host/lease.rs:2313`, `rh/host/lease.rs:3710`, `rh/host/lifecycle.rs:622`, `rh/host/lifecycle.rs:675`, `rh/host/online_observation.rs:519`, `rh/host/planning.rs:82`, `rh/host/policy_catalog.rs:240`, `rh/host/policy_catalog.rs:263`, `rh/host/policy_catalog.rs:327`, `rh/host/policy_dispatch.rs:642`, `rh/host/policy_dispatch.rs:1511`, `rh/host/policy_outcome.rs:54`, `rh/host/policy_outcome.rs:1687`, `rh/host/policy_outcome.rs:1757`, `rh/host/policy_outcome.rs:2038`, `rh/host/runtime_facts.rs:1123`, `rh/host/runtime_facts.rs:1239`, `rh/host/state_control.rs:46`, `rh/host/state_control.rs:119`, `rh/host/state_control.rs:140`, `rh/host/state_control.rs:168`, `rh/host/state_control.rs:297`, `rh/host/state_control.rs:324`, `rh/host/state_control.rs:370`, `rh/ledger_maintenance.rs:311`, `rh/monitor.rs:88`, `rh/monitor.rs:174`, `rh/owner_unlock.rs:108`, `rh/performance/capacity.rs:698`, `rh/policy_host/catalog_transaction.rs:268`, `rh/provider_startup.rs:48`.
- Calls of the `f` facade (`append_event`, `append_event_raw`, `append_event_observed`, `append_event_under_fact_gate`; the forwards inside `facts.rs` excluded): **135**.
- `append_event_under_fact_gate` calls: **14** — `rh/host/backend_open.rs:51`, `rh/host/contained_task.rs:6677`, `rh/host/contained_task.rs:6693`, `rh/host/contained_task.rs:7237`, `rh/host/contained_task.rs:7250`, `rh/host/contained_task.rs:7300`, `rh/host/contained_task.rs:7395`, `rh/host/facts.rs:352`, `rh/host/lifecycle.rs:79`, `rh/host/lifecycle.rs:303`, `rh/host/runtime_facts.rs:314`, `rh/host/runtime_facts.rs:632`, `rh/host/runtime_facts.rs:831`, `rh/host/runtime_facts.rs:898`.
- Rows marked D1c: **33**.

Comparison with the model (§3.3, §4.1a, §4.1b; figures from research A at `23bf771e` and the v1.2 recheck at `010435c3`):

| model figure | what it counts | measured | explanation |
|---|---|---|---|
| "~190 by file (git grep, incl. wrapper definitions)" | lines of a narrow grep | `git grep -E "append_event[a-z_]*\(\|\.append\(\|append_transaction\(\|append_deferred\(\|append_durable\(\|execute_critical\("` over non-test files: **194** at `010435c3`, **203** at HEAD | Reproduced. That grep counts lines, so it also counts `Vec::append`, inline `#[cfg(test)]` modules and the `f` forwards, and it does not see the named wrapper families (`append_lease_requested`, `append_lifecycle_failure`, `append_recovery_ladder_fact`, `record_runtime_fact*`, `ProviderStartup::record` …) or their call sites. This table follows those wrappers, so it has 461 rows (398 call sites + 63 definitions). |
| "~125 via `f`" | calls of the `f` facade | **135** at HEAD (121 without the 14 under-gate calls); **126** at `010435c3` (112 without them) | Matches at `010435c3` when the under-gate calls are included. The nine calls added since then: emulator control +3 (`emulator_instance.rs`), unreleased-run recovery and dispatch diagnostics +3 (`policy_outcome.rs:295/307/492`), frame retention warning +1 (`frame_retention.rs:120`), worker panic +1 (`host_claims.rs:209`), lease +1. |
| "~27 direct" | sites in the §4.1a table | **27** sites in §4.1a if the settlement continuation (`po:52/:1203/:1495`) and W2 are left out; **31** with them. HEAD has **48** DIRECT call sites | 48 = 2 calls that *are* the `f` layer (`f:612`, `f:616`) + 32 calls for the 31 §4.1a sites (`pd:1471` via `pd:605` is two calls at HEAD, `pd:1511` and `pd:642`; moved lines in §6) + **14 calls §4.1a misses** (§4, M13). |
| "14 `append_event_under_fact_gate`" | under-gate calls | **14** (list above) | Matches; same 14 at `010435c3` (`ct` lines moved: 6559→6677, 6575→6693, 7142→7237, 7155→7250, 7205→7300, 7300→7395; `lifecycle.rs` 76→79, 300→303). |

### A.4 Misfits, model omissions, and proposals

**M1 — G16 mixes A and B, and most `append_lifecycle_failure` calls are not process-fatal.** §4.1 names `lifecycle.rs:113-126` (`append_lifecycle_failure`, now `:116-129`) as the G16 "fatal" choke point. At HEAD the function has 27 callers in runtime-host and 7 in actingd; 21 of the runtime-host callers pass operation-level stages (`operation_cleanup`, `session_close`, `connection_preparation`, `connection_cleanup`); only actingd's process stages (`policy_initialization`, `policy_bootstrap`, `policy_monitor`, `policy_control`, `retained_reference`, `host_close`) and the install stages are process lifecycle. `runtime.failed` has no business reader (only `suspension_report.rs:143` and the views), so these rows are **B**, while §4.1b lists G16 among the A gates. Proposal: keep `append_lifecycle_failure_with_severity` (`lifecycle.rs:129`) as the single `runtime.failed` choke point (one capture call), class B, and let G16 cover only the process lifecycle records (`runtime.started/takeover/instance_bound`, shutdown phases, the device diagnostic summary, `prior_epoch_owner_released_by_exit`), which are A except the summary. The connection-preparation failures (`resource_close.rs:1004`, `:1086`) carry the backend-open codes that D1c counts; the counter must see them through the same choke point.

**M2 — `provider.startup_observed` has no gate and is not in §4.1a.** `rh/provider_startup.rs:48` writes directly (`GlobalLedger::append`); the handle is given to actingd's provider assembly (`rh/host.rs:1567`), which calls it from 25 sites in `ad/config/provider_startup.rs` (18 `record`, 7 `failed`). Startup-only, before business opens, no instance link, observational today. Proposal: **G10** (backend and self-check observations), captured in the startup segment; class B. Because it has no instance link, the D1c per-instance counter does not count it.

**M3 — host-triggered records that the ledger drafts itself.** `rh/host.rs:1260` (`reconcile_prior_epoch_closes` → `prior_epoch_owner_imported` / `prior_epoch_scope_closed`) and the startup eviction recovery `rh/host/frame_retention.rs:702` / `:709` (`admit_artifact_eviction` / `finish_artifact_eviction` → `artifact.eviction_intent` / `eviction_outcome`). §6.1 counts retention and eviction (`g:2030-2084`, now `global.rs:2155-2209`) as the ledger's own records, but at HEAD these three are started by the host (startup segment, `host.rs:1183` calls `FrameRetention::recover`). Proposal: treat them as ledger-own records (not captured, no `observe_trigger`, added to the `invalidate_on` refusal list with the other ledger-own types), and put the three call sites on the AO-3 whitelist as "ledger-own triggers". The §4.8 write-API list (`append*`, `execute_critical`, `reconcile_*`, `import`) misses `admit_artifact_eviction`, `finish_artifact_eviction` and `release_lab_pin` (M4); the guard should list every writing `GlobalLedger` method instead of name patterns.

**M4 — `release_lab_pin` (`rh/host/client_events.rs:21`).** The host first records the request (`lab.request`, `:11`), then calls `GlobalLedger::release_lab_pin`, which drafts `artifact.pin_released` inside the writer. Not in §4.1a, not matched by the guard patterns. Proposal: **G9** (it is the pin side of `artifact.*`), or treat as M3 (ledger-drafted). Category decision (not in today's observational list).

**M5 — recovery ladder records have no gate.** 17 rows in `rh/host/recovery_ladder.rs` write `runtime.lifecycle_observed` phases (`recovery_ladder_started/finished/suppressed`, `recovery_rung_finished`, `instance_preparation_finished`) through `append_recovery_ladder_fact` → `append_event_raw`. Nothing in the host reads them back (the ladder window is in memory, `host.rs:3798`), so class B; durable today. None of G1–G16 fits: the ladder runs as host-claim work on the instance queue. Proposal: **G4** (the claim's queue work trace) or a new ladder gate; to be decided with D1c, whose suppression record is one of these rows. The ladder's emulator restart and application stop (`:1135`, `:1204`, `:1257`, `:1288`) fit G13/G14 and are placed there.

**M6 — read-only command audit has no choke point.** §4.1b lists "read-only command audit" as B, G14 says read-only requests are not copied, and G7–G11 have no request-record entry. Rows: `rh/host/observation.rs:87`, `:131`, `rh/host/online_observation.rs:87` (client record + `command.received` + `command.validated` for read-only observe, capture sequence, contained page observe). Proposal: **G8** at the observation entry (the request is part of the observation pipeline), or stop them like the status query. The status query itself (`rh/host/read_events.rs:363`) is `stop` per §6.1.

**M7 — planning IPC markers.** `rh/host/planning.rs:181`, `:234`, `:275` write `strategic_report_entered`, `policy_forward_entered` and the returned phase for read-only planning requests. No gate, no reader. Proposal: stop (as the status query) or keep under G15 with the planning signal; failures (`:276`) stay with M1.

**M8 — guard name patterns over-match.** `append*` also matches `Vec::append` (`rh/agent_dispatcher.rs:403`, `rh/host/performance.rs:377`), the owner journal (`rh/owner.rs` `append_in_epoch` / `append_record`) and the task diagnostic file stream (`rh/host/task_diagnostic.rs`); `reconcile_*` also matches nine host functions that are not ledger calls (`reconcile_performance_control/tick`, `reconcile_agent_wakes`, `reconcile_runtime_state`, `reconcile_release_state`, `reconcile_policy_dispatches`, `reconcile_scheduled_policy_outcomes(_for)`, `reconcile_pending_policy_settlements`). The AO-3 guard should match calls on a `GlobalLedger` / `LedgerMaintenance` receiver (or the `EventAppender` trait), not bare names.

**M9 — business readers of A records that §3.6 does not list.** (a) Monitor registry: `MonitorRegistry::recover` reads the ledger at startup (`rh/monitor.rs:100-180`, `query_page` at `:129`) and writes `command.validated` (`monitor_imported`, `:174`) and a Fatal `runtime.failed` (`:88`) directly; probe results are applied to the registry (`monitor_control.rs:435/480/617/643`). (b) Signature catalog: `rh/host/signatures.rs:15-23` rebuilds the catalog from the ledger on every register / retire / match. Both need a decision-memory or tagged-record source like the stores §3.6 lists. (Agent wake reconciliation, `agent_control.rs:221-275`, is already in §3.6 as `agent_control.rs:229-240`.)

**M10 — G7 category.** G7 says "terminal and `task.selection_evaluated` decision, the rest observation". By the rule used here (decision = durable today) `task.requested`, `task.started`, `task.step_started/finished`, `task.entry_preflight`, `task.effect_intent/completed` stay decision; only `task.evidence_indexed`, `task.geometry_observed`, `task.recognition_started/completed` are observation. AO-0 has to pick one; the table shows today's class.

**M11 — family vs choke point.** `capture.summary_committed` (`ct:7237`) is a capture-family record written at the task terminal choke point (placed in G7, decision). `monitor.recovery_admitted/deferred` (`monitor_control.rs:534`) are monitor-family (B) but are scheduling admissions (decision). Task records of Lab debug runs come from IPC, not the kernel (`client_events.rs:121-137`, `package_debug.rs:114/125`, `evidence_export.rs:64/72`; placed in G7). `runtime.fact_recorded` writes of self-check values (`rf:300`) are A/G3 while the paired `device.self_check` record (`rf:314`) is B/G10.

**M12 — "uses the returned sequence" is common.** Beyond the cases §3.4 names, the result's sequence flows into receipts (`terminal()` → `TerminalEvent`, 52 rows), into in-memory stores (agent dispatcher `event_sequence`, monitor registry `last_sequence`, approval projection keyed by sequence, `FactPublication.sequence`, fact store `acknowledge_generated_invalidation`, capacity `CapacityFactReference`, `committed_signal`, effective-configuration `source_sequence`, `PolicyHost` intent/completion sequences), and into the status reply. Every row whose note mentions a receipt, a sequence or an apply (68 rows) is a §3.4 change point.

**M13 — writes §4.1a missed** (all present at `010435c3` too; base line in brackets):

| HEAD site (`010435c3` line) | writes | proposed gate |
|---|---|---|
| `rh/host/agent_control.rs:324` (324) | `agent.wake_requested` | G12 |
| `rh/monitor.rs:88` (88) | `runtime.failed` (monitor restore, Fatal) | G16 |
| `rh/monitor.rs:174` (174) | `command.validated` (`monitor_imported`) | G16 (startup) |
| `rh/provider_startup.rs:48` (48) | `provider.startup_observed` | G10 (M2) |
| `rh/host/runtime_facts.rs:1123` (1120) | `lifecycle_observed` `fact_snapshot_set_skipped` | G16 (startup) |
| `rh/host/runtime_facts.rs:1239` (1236) | `runtime.fact_invalidated` (takeover) | G3 |
| `rh/host/state_control.rs:119` (119) | `release.transition_intent` (G15 cites the plan line `:113`) | G15 |
| `rh/host/state_control.rs:297` (297) | `state.migrated` (startup reconcile) | G15 |
| `rh/host/policy_catalog.rs:240` (240) | `catalog.transition_intent` (G15 cites the plan line `:232`) | G15 |
| `rh/host/policy_outcome.rs:1757` (1262) | `policy.dispatch_rejected` (startup reconcile) | G5 |
| `rh/host.rs:1260` (1145) | prior-epoch close records | ledger-own (M3) |
| `rh/host/frame_retention.rs:702`, `:709` (274, 281) | eviction intent / outcome (HEAD: startup recovery `EvictionRecovery::round`; base: the regular `FrameRetention::round`) | ledger-own (M3) |
| `rh/host/client_events.rs:21` (21) | `artifact.pin_released` | G9 (M4) |

`policy_dispatch.rs:1511` (`execute_critical`) and `:642` (`append_durable` in `PolicyAdmissionAppender`) are one §4.1a row ("`pd:1471` via `pd:605`"); both are listed here.

### A.5 The exception table (W) at HEAD

Both W rows are in runtime-host but are reached only from actingd's offline subcommands; no code path of the running actingd reaches them.

| # | site at HEAD | writes | entry chain |
|---|---|---|---|
| W1 | `rh/owner_unlock.rs:108` (`ledger.append` in `fn record`, `:60`; writer opened at `:72-88` with `LedgerMaintenance::acquire` + `open_writer`) | `cli.command` with payload variant `OwnerUnlock` (the model's "`client.owner_unlock`" is the variant name, not an event type) | `ad/main.rs:111-116` (`unlock-owner`) → `ad/owner_unlock.rs:49` `RuntimeHost::unlock_owner` → `rh/host.rs:1006-1011` → `rh/owner_unlock.rs:34` `run` → `:48` `record(...)` → `:108` |
| W2 | `rh/ledger_maintenance.rs:311` (`maintenance.import(...)`; completion draft built at `:303`) → `LedgerMaintenance::import` (`crates/ledger/src/global/migration.rs:271`) | `ledger.recovered` (`LedgerPayloadDraft::migrated`, reason `storage_cutover`, with the migration record) | `ad/main.rs:99-104` (`ledger-maintenance`; `:84-91` only selects the install purpose for `verify`) → `ad/ledger_maintenance.rs:60` `RuntimeHost::maintain_ledger` → `rh/host.rs:993-1003` → `rh/ledger_maintenance.rs:148` `run` → `:168` `run_locked` (`:193`) → `:303`/`:311` |

- W1 and W2 sit exactly where the model says (`owner_unlock.rs:108`, `ledger_maintenance.rs:303-311`); the owner lock for W2 is still taken at `:163-165` and `restore` still only copies (`:545`).
- `dry-run` passes `dry_run = true` into the same `import` call, which rolls the import transaction back (`crates/ledger/src/global/sqlite.rs:656-659`), so nothing is written; `verify` and `backup` do not reach `:311`.
- No other actingd subcommand writes the ledger. actingd's other writes all go through the running host: `ProviderStartup` (25 sites, M2) and `RuntimeHost::record_lifecycle_failure` (7 sites, M1), both in the table, plus business calls into the host API (`apply_policy_catalog_transition`, `admit_policy_dispatch`, `admit_scheduled_policy_dispatch`, `run_scheduled_contained_task`, `complete_scheduled_policy_run`, approvals over IPC at `ad/main.rs:278`), whose ledger writes are the host rows above.

### A.6 Model line citations: `010435c3` → `706ead94`

| model citation | HEAD | note |
|---|---|---|
| `f:511-557`, `f:552`, `f:666`, `f:196-374`, `f:86-167` | unchanged | |
| `rf:586-646`, `rf:799-853`, `rf:237-345` | unchanged | |
| `installation.rs:245-344` | `:247-346` | |
| `ls:1297/:1304` | `:1300/:1307` | grant |
| `ls:1654/:1661` | `:1657/:1664` | release |
| `ls:2129/:2134` | `:2308/:2313` | cleanup (expire / release) |
| `ls:1494/:1501` | `:1497/:1504` | renew |
| `ls:3308/:3315` | `:3703/:3710` | renew held |
| `pd:1416`, `pd:1471` | `:1456`, `:1511` | |
| `pd:605` (`append_durable`) | `:642` (impl `:638`) | the ladder model's `policy_dispatch.rs:640` is the same call |
| `input.rs:182-195`, `:172/:197`, `:500/:510` | `:184-197`, `:174/:199`, `:502/:512` | |
| `input.rs:701`, `:860` | `:703`, `:845` | readers |
| `ct:919`, `ct:3254` (kernel `record` impls) | `:1045`, `:3380` | |
| `ct:1891` | `:2017` | `append_task` |
| `ct:7069` (`TaskTerminal`) | `:7154` | `append_contained_task_terminal` |
| `ct:3946-3960`, `ct:3964` | `:4074-4100`, `:4093` | the test sink is now `:4070-4082`; the real impl `:4084-4100`; the ladder model's `contained_task.rs:4077` is this sink |
| `ct:6663-6672` | `:6781-6790` | text moved; the recovered-terminal writer is `:6658-6700` |
| `online_observation.rs:519` | unchanged | |
| `frame_retention.rs:156` | `:210` | |
| `backend_open.rs:8-29` | unchanged | |
| `lifecycle.rs:31`, `:48`, `:113-126`, `:619`, `:672` | `:34`, `:51`, `:116-129`, `:622`, `:675` | |
| `hp:413-450`, `hp:436-444` | unchanged (append at `:440`) | |
| `performance/capacity.rs:698` | unchanged | |
| `agent_control.rs:144-336` | unchanged | |
| `emulator_instance.rs:105-128` | `:298-331` + `:496-523` | restructured; the start/stop result writes are in `drive_control_phase` (`:524`, writes `:707`, `:721`) |
| `req:11-120` | `requests.rs:11-128` | |
| `governance.rs:97`, `pl:82` | unchanged | |
| `state_control.rs:46/:113/:140/:168/:324/:370` | unchanged | `:113` is the plan line; the intent append is `:119` |
| `policy_catalog.rs:232/:263/:327` | unchanged | `:232` is the plan line; the intent append is `:240` |
| `policy_host/catalog_transaction.rs:268` | unchanged | |
| `device_diagnostic.rs:205` | unchanged | |
| `owner_unlock.rs:108`, `:72-88` | unchanged | |
| `ledger_maintenance.rs:303-311`, `:163-165`, `:545` | unchanged | |
| `re:363-379`, `re:317-399` | unchanged | |
| `po:52`, `po:1203`, `po:1495` | `:54`, `:1687`, `:2038` | settlement continuation calls |
| `h:1543`, `h:1550` | `:1713`, `:1720` | |
| `h:3029-3048` | `:3326-3345` | shutdown requested |
| `h:3752` | `:4066` | `finish_device_diagnostics` at close |
| `h:3382`, `h:3460-3464` | `:3687`, `:3769-3773` | lock-order comment, queue order locks |
| `h:1864-1885` | `:2028-2049` | release set API |
| `g:1599` | `crates/ledger/src/global.rs:1706` | |
| `g:2030-2084` | `global.rs:2155-2209` | eviction admission and outcome in the writer loop |
| `migration.rs:271`, `artifact-store store.rs:22`, `execution-kernel contained_task.rs:1773` | unchanged | |

### A.7 Artifact sink construction sites (G9 feeders)

The two `ArtifactEventSink` impls (`ct:4085`, `online_observation.rs:511`) are called by the artifact store, not by host code; their records originate where a sink is built and passed to `ArtifactStore::put`:

- `RuntimeArtifactEventSink` (14): `ct:805`, `:1694`, `:1769`, `:1876`, `:2183`, `:2578`, `:2586`, `:2940`; `evidence_export.rs:151`; `monitor_control.rs:339`; `observation.rs:424`; `planning.rs:790`, `:819`; `task_diagnostic.rs:876`; (`ct:1694` is the effective-configuration record).
- `ObservationArtifactSink` (9): `ct:796`, `:1778`, `:1881`, `:2931`; `observation.rs:393`; `online_observation.rs:322`, `:490`; `ppocr_diagnostic.rs:302`; `saved_artifact_ocr.rs:366`; plus the frame pin it triggers (`frame_retention.rs:210`).

`artifact.created` and (new) `artifact.verified` are decision; `artifact.pin_recorded` is observation; the planning reader `read_verified_artifacts` (`pl:18-40`) is the X6 consumer of `artifact.verified`.

### A.8 AO-0 decisions on the misfits

The survey above records today's code and proposals. AO-0 decides as follows; the main text
(§4, §6) follows these decisions, and the coordinator may overrule each one.

| Misfit | Decision |
|---|---|
| M1 | Every `runtime.failed` has one choke point, `append_lifecycle_failure_with_severity` (`lifecycle.rs:129`), and stays under G16. Records of operation-level stages (cleanup, session close, connection preparation) are class B; process lifecycle records are A. The ladder's direction-1 counter (D1c) sees the connection-preparation failures through the same choke point. |
| M2 | `provider.startup_observed` (`provider_startup.rs:48`, 25 actingd callers) → G10, class B, captured during start (the capture queue runs from step 1, §12). |
| M3 | The prior-epoch imports and scope closes (`host.rs:1260`) and the startup eviction recovery (`frame_retention.rs:702`, `:709`) are drafted inside the ledger writer: they are the ledger's own records, not captured. Their three call sites are "ledger-own triggers" on the AO-3 whitelist beside the exception table. `artifact.eviction_intent` / `eviction_outcome` are already refused as `invalidate_on` triggers; the prior-epoch phases are `runtime.lifecycle_observed` records written before step 6, before the stores load, so no loaded fact can be waiting on them. |
| M4 | `artifact.pin_released` (`release_lab_pin`, `client_events.rs:21`) → G9, decision. |
| M5 | The recovery ladder's records go through `append_event_raw`, i.e. the fan-out point → G1 (as SCHED §11 places the ladder suppression record), class B, durable category as today. |
| M6 | Read-only requests that write records (observe, capture sequence, contained page observe) → G14: Workflow #389 §5.2 copies the command records of reads that write records, with `channel`. Class B. |
| M7 | The planning IPC markers (`planning.rs:181`, `:234`, `:275`) are the planning projection's own records (Workflow #389 line 956: "规划投影各写自己的记录，另抄一条带通道的命令记录"): copied at the planning entry, with G14 for the command record; class B. Stopping them is a behaviour change outside AO-0. |
| M8 | The AO-3 guard keys on calls whose receiver is `GlobalLedger`, `LedgerMaintenance` or the capture and barrier traits, not on name patterns. Its write API list adds `admit_artifact_eviction`, `finish_artifact_eviction`, `release_lab_pin`, `reconcile_prior_epoch_closes` and `reconcile_scheduled_policy_settlement`. |
| M9 | Monitor registry: LEDG §10.1 already moves its start read to `monitor.journal`; probe results stay in memory. Signature catalog (`signatures.rs:15-23`, rebuilt from the ledger on every register, retire and match): not covered by LEDG §3.6. Open for AO-4: an exception read of one type (like X6, after a flush barrier) or tagged value records; reported to the coordinator. |
| M10 | G7 categories: decision for the terminal family (`task.completed`, `task.failed`, `task.cancelled`, `task.terminal_intent`, `task.terminal_commit_failed`, `task.terminal_rejected`), `task.selection_evaluated`, and `task.requested` / `task.started` (X1 reads open runs; LEDG §4.1: a type a business reader consumes is a decision record); observation for the rest (`task.step_*`, `task.effect_*`, `task.entry_preflight`, `task.evidence_indexed`, `task.geometry_observed`, `task.recognition_*`). The table above shows today's class. |
| M11 | A record is copied by the gate of the choke point where it is written; its category follows its type. |
| M12 | Every row that puts the returned sequence into a receipt, a store or a reply (68 rows) is a §13 change point for AO-3 / AO-4. |
| M13 | The 14 writes §4.1a missed join the gates listed in A.4. |

