// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-8d (brief rd-1026/brief-open-stall.md; the v0.12.14 start on the production ledger,
// 2026-10-26): the writer open reads the retention state of the frames in the live frame store
// (LN-7) by link and by index. On the production ledger 5 638 live frames, from sequence 101 557
// to the head 1 579 741, name 1 323 runs whose records number 402 782; v0.12.14 read each link
// dimension of them in pages of 256 whose statement sorted every linked record after the page in
// a temporary B-tree, so the open's work grew with the square of those records (about 720 GB
// read for the runs alone, the start stalled), and the open's deadline was not checked inside
// that read.
//
// The roots hold the synthetic ledger (LN-1) with capture frames stored through the root's
// artifact store (`frame_artifact_every_frames`), so they are live in its frame store from the
// start of the history to its end, as production's are:
// - the open's SQLite work per record it reads does not grow with the history (it grew with the
//   linked records after each page). #422 LN-7c Q removed the open's read of the live frames, so
//   the rows the open reads no longer grow with the history either (adapted: this test required
//   them to grow about fourfold, as LN-7's frame read made them);
// - the open's deadline stops its retention read with `ledger_read_budget_exceeded`.

use crate::global::{
    GlobalLedger, GlobalLedgerError, LedgerFailurePlan, LedgerMaintenance, LedgerOpenTiming,
    LedgerWritePolicy,
};
use crate::{
    LedgerOpenDelayGate, LedgerReadCounter, SyntheticInstance, SyntheticLedgerSpec,
    write_synthetic_ledger,
};
use actingcommand_contract::{IdentifierIssuer, OwnerEpoch};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

// The State key (a bootstrap seed of at least 16 bytes), the writer's owner id and the fixture
// instance: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-ln8d-fixture";
const OWNER: &str = "ledger-ln8d-fixture";
const INSTANCE_ID: &str = "\"instance_000000000000000000000000000000a1\"";
// Two histories of the same shape, one four times the other.
const SMALL_RECORDS: u64 = 3_000;
const LARGE_RECORDS: u64 = 12_000;
// Every sixth frame of a run is stored and stays live: live frames in every run.
const LIVE_FRAME_EVERY: u64 = 6;
// The open authenticates a short tail, so its reads are mostly the retention state's.
const TAIL: usize = 64;
// The records' time, for the write policy's epoch.
const AT_MS: u64 = 1_752_147_200_000;
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
// The opens' deadlines (`MaintenanceLimits::timeout_seconds`): one no open here reaches, and the
// one the index delay below passes.
const OPEN_TIMEOUT_SECONDS: u64 = 600;
const DEADLINE_SECONDS: u64 = 60;
const INDEX_DELAY: Duration = Duration::from_secs(DEADLINE_SECONDS + 1);

// A closed formal root holding `records` synthetic records with live stored frames.
struct Root {
    dir: TempDir,
    database: Arc<RuntimeDatabase>,
    live: u64,
}

fn root(records: u64) -> Root {
    let dir = tempfile::tempdir().expect("state root");
    let database =
        Arc::new(RuntimeStateStore::open_database(dir.path(), STATE_KEY).expect("State database"));
    let limits = MaintenanceLimits::default();
    let maintenance = LedgerMaintenance::acquire(
        dir.path(),
        true,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("maintenance owner");
    maintenance
        .initialize_empty(&database)
        .expect("formal ledger");
    maintenance
        .open_writer(Arc::clone(&database), OWNER.into())
        .expect("formal writer")
        .close()
        .expect("close the formal writer");
    let mut spec = SyntheticLedgerSpec::new(
        records,
        vec![SyntheticInstance {
            alias: "fixture-instance-a".to_owned(),
            instance_id: serde_json::from_str(INSTANCE_ID).expect("a fixture instance id"),
        }],
    );
    spec.mix.frame_artifact_every_frames = LIVE_FRAME_EVERY;
    let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
    assert_eq!(generated.records, records, "{generated:?}");
    let live = generated.stored_frames;
    assert!(
        generated.runs > 4 && live > 2 * generated.runs,
        "live frames throughout the history: {} runs, {live} live frames",
        generated.runs
    );
    Root {
        dir,
        database,
        live,
    }
}

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

// The deadline `timeout_seconds` from now, as `MaintenanceLimits` sets one.
fn deadline_after(timeout_seconds: u64) -> Instant {
    MaintenanceLimits {
        timeout_seconds,
        ..MaintenanceLimits::default()
    }
    .deadline()
    .expect("deadline")
}

// A start's writer open of `root` with a tail of `TAIL` records, before `deadline`.
fn open(
    root: &Root,
    deadline: Instant,
    timing: &mut LedgerOpenTiming,
) -> Result<GlobalLedger, GlobalLedgerError> {
    let state_root = root.dir.path().canonicalize().expect("state root");
    let plan = LedgerFailurePlan::new(
        LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
            .expect("write policy"),
        &state_root,
        None,
        epoch(),
        AT_MS,
    )
    .with_open_verify_tail_records(TAIL);
    LedgerMaintenance::acquire(
        root.dir.path(),
        false,
        MaintenanceLimits::default(),
        deadline,
    )?
    .open_writer_timed(Arc::clone(&root.database), OWNER.into(), Some(plan), timing)
}

// The open of `root`, counted on this thread (the opening thread): the SQLite virtual-machine
// steps of its statements and the ledger rows it read.
fn counted_open(root: &Root) -> (u64, u64) {
    let counter = LedgerReadCounter::start();
    let ledger = open(
        root,
        deadline_after(OPEN_TIMEOUT_SECONDS),
        &mut LedgerOpenTiming::default(),
    )
    .expect("the writer opens");
    let counts = counter.counts();
    let steps = counter.vm_steps();
    drop(counter);
    ledger.close().expect("close the writer");
    let rows = counts.events.rows + counts.links.rows + counts.artifacts.rows;
    assert!(rows > 0 && steps > 0, "{steps} VM steps, {counts:?}");
    (steps, rows)
}

#[test]
fn ln8d_the_opens_work_per_record_read_does_not_grow_with_the_history_of_its_live_frames() {
    let small = root(SMALL_RECORDS);
    let large = root(LARGE_RECORDS);
    let (small_steps, small_rows) = counted_open(&small);
    let (large_steps, large_rows) = counted_open(&large);
    // #422 LN-7c Q: the open reads no frame, so the history four times as long, with four times
    // the live frames, adds nothing to what it reads.
    assert!(
        large_rows * 2 <= small_rows * 3,
        "the open read {small_rows} rows of {SMALL_RECORDS} records ({} live frames) and \
         {large_rows} of {LARGE_RECORDS} ({} live frames)",
        small.live,
        large.live
    );
    // Its SQLite work per row read stays the same (a sort of every linked record after each
    // page made it grow with the records: four times as many here).
    assert!(
        large_steps * small_rows * 2 <= small_steps * large_rows * 3,
        "VM steps per row read: {small_steps} / {small_rows} on {SMALL_RECORDS} records, \
         {large_steps} / {large_rows} on {LARGE_RECORDS}"
    );
}

#[test]
fn ln8d_the_opens_deadline_stops_its_retention_read() {
    let root = root(SMALL_RECORDS);
    let deadline = deadline_after(DEADLINE_SECONDS);
    {
        // The `index` phase starts past the deadline; `meta` and `tail` run at their own speed.
        let _late =
            LedgerOpenDelayGate::arm_index(root.dir.path(), INDEX_DELAY).expect("the index delay");
        let mut timing = LedgerOpenTiming::default();
        let error = open(&root, deadline, &mut timing)
            .expect_err("the open stops at its deadline in its retention read");
        assert_eq!(error.code(), "ledger_read_budget_exceeded", "{error:?}");
        assert_eq!(error.operation(), "open_runtime_ledger", "{error:?}");
        assert_eq!(
            timing.events, TAIL as u64,
            "the tail was authenticated first: {timing:?}"
        );
    }
    // Without the delay the same root opens within the same deadline.
    open(&root, deadline, &mut LedgerOpenTiming::default())
        .expect("the writer opens")
        .close()
        .expect("close the writer");
}
