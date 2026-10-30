// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-OFF-1 (design rd-1029/design-ln-off.md option A, §6.3 T1 to T7; brief
// rd-1029/brief-ss5-lnoff1.md; pending decision 115): `GlobalLedger::open_metadata` on an SQLite
// root is bounded. The UI's offline face reached 12 GB in 60 s on the 8.1 GB production ledger:
// the opening read and authenticated the whole ledger, and every page did it again.
// - T1: the opening reads the same rows on 3 000 and on 12 000 records: the meta row, the
//   aggregates and the newest `open_verify_tail_records` (256 here) records, and no older record;
// - T2: each view's first page and its next page read the same rows on both histories, and the
//   two pages read no row twice but the lookahead row that ends a full page;
// - T3: a page of the offline snapshot is the writer's page at the same point, field by field,
//   but for its source (`offline`);
// - T4: damage the writer open refuses is refused by the offline opening with the same code and
//   operation; a damaged older record is refused by the page that reads it;
// - T5: with the defaults alone, a deadline that passes refuses the opening and a page, between
//   statements and inside one, with `ledger_read_budget_exceeded` naming the phase; an opening's
//   deadline does not bound the later reads;
// - T6: on a root whose writer appends meanwhile, the pages stay at the pinned head, the appends
//   commit, and a passive checkpoint reaches the end of the WAL after the pages;
// - T7: a material fetch gives the eviction proof and availability the whole replay gave;
// - the bound (Alice, pending decision 115): a page returns at most its limit, with a cursor that
//   reads the rest.
// Every deadline is judged by the fake read clock (`LedgerReadClock`) or lies far ahead; nothing
// sleeps. No production data: synthetic ledgers and fixture records only.

use super::missing_material::{Capture, capture, formal_root, material_store};
use super::*;
use crate::global::{
    ArtifactEvictionAdmission, ArtifactEvictionPermit, GlobalLedgerError, GlobalLedgerMetadata,
    LedgerArtifactSelection, LedgerFailurePlan, LedgerMaintenance, LedgerOpenTiming,
    LedgerWritePolicy,
};
use crate::{
    LedgerReadClock, LedgerReadCounter, SyntheticInstance, SyntheticLedgerSpec,
    write_synthetic_ledger,
};
use actingcommand_artifact_store::{ArtifactStore, try_artifact_delete_guard};
use actingcommand_contract::{
    LedgerEventPosition, LedgerPageLimit, LedgerReadSource, LedgerView,
    MAX_RUNTIME_EVENT_QUERY_EVENTS, OwnerEpoch, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};
use actingcommand_runtime_state::RuntimeStateStore;
use std::collections::BTreeSet;
use std::sync::Arc;

// The State key (a bootstrap seed of at least 16 bytes), the writer's owner id and the fixture
// instance: neutral fixture words.
const STATE_KEY: &[u8] = b"ledger-ln-off1-fixture";
const OWNER: &str = "ledger-ln-off1-fixture";
const INSTANCE_ID: &str = "\"instance_000000000000000000000000000000c1\"";
// Two synthetic histories of the same shape, one four times the other.
const SMALL_RECORDS: u64 = 3_000;
const LARGE_RECORDS: u64 = 12_000;
// The offline opening's tail on the synthetic roots, shorter than both histories.
const SYNTHETIC_TAIL: usize = 256;
// A page of the equivalence and live-root tests.
const PAGE_LIMIT: u16 = 50;
// A page of the scale test (T2): two pages of each view, with their lookahead, lie within the
// records both histories share. The generator writes the same records whatever the length, but
// cuts the small history's last run short at its head and ends it with filler records; of its 76
// Changes records the first 71 (through record 2 961) are the large history's too. Two pages of 50
// read 102 Changes rows, more than the small history holds: its next page ended at its head with
// 26 rows where the large one's was full with 51. Two pages of 30 read 62 rows, through record
// 2 461, on both.
const SCALE_PAGE_LIMIT: u16 = 30;
// The plain roots of the damage and deadline tests: their records and tail, and a record well
// before the tail (as `gate_ln8.rs`).
const PLAIN_RECORDS: u64 = 40;
const PLAIN_TAIL: usize = 8;
const OLD: u64 = 5;
// The writer open's write policy (as `gate_ln8.rs`) and the records' time.
const AT_MS: u64 = 1_752_147_200_000;
const STALL_MS: u64 = 120_000;
const PROBE_MS: u64 = 1_000;
const DUMP_MAX_BYTES: u64 = 1_073_741_824;
// T6: records before the offline opening and records the writer appends while it pages.
const LIVE_BEFORE: u64 = 300;
const LIVE_APPENDED: u64 = 200;
// The bound: a history longer than three full pages.
const BOUND_RECORDS: u64 = 3 * MAX_RUNTIME_EVENT_QUERY_EVENTS as u64 + 40;
// A deadline far ahead of any read here.
const ENOUGH_SECONDS: u64 = 600;

/// The deadline `seconds` from now, as `MaintenanceLimits` takes one (a gate file reads no clock).
fn deadline_after(seconds: u64) -> Instant {
    MaintenanceLimits {
        timeout_seconds: seconds,
        ..MaintenanceLimits::default()
    }
    .deadline()
    .expect("a deadline")
}

/// A deadline that has passed: the moment one `ENOUGH_SECONDS` ahead was taken.
fn deadline_passed() -> Instant {
    deadline_after(ENOUGH_SECONDS)
        .checked_sub(Duration::from_secs(ENOUGH_SECONDS))
        .expect("the moment the deadline was taken")
}

fn epoch() -> OwnerEpoch {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport()
}

/// A sequence as the ledger tables store it.
fn stored(sequence: u64) -> i64 {
    (sequence ^ (1_u64 << 63)) as i64
}

/// A closed formal root holding `records` synthetic records (LN-1).
fn synthetic_root(records: u64) -> TempDir {
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
    let spec = SyntheticLedgerSpec::new(
        records,
        vec![SyntheticInstance {
            alias: "fixture-instance-c".to_owned(),
            instance_id: serde_json::from_str(INSTANCE_ID).expect("a fixture instance id"),
        }],
    );
    let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
    assert_eq!(generated.records, records, "{generated:?}");
    dir
}

/// A formal root of `records` plain records, its writer closed.
fn plain_root(records: u64) -> (TempDir, Arc<RuntimeDatabase>) {
    let dir = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(dir.path());
    for index in 0..records {
        ledger
            .append(event(&format!("plain-{index}")))
            .expect("plain record");
    }
    ledger.close().expect("close the writer");
    (dir, database)
}

/// The bounded offline opening with the defaults and a tail of `tail` records.
fn offline(root: &Path, tail: usize) -> Result<GlobalLedgerMetadata, GlobalLedgerError> {
    GlobalLedger::open_metadata(
        GlobalLedgerEvidenceConfig::new(root).sqlite_open_verify_tail_records(tail),
    )
}

/// A first page of `limit` events of `view` (all events without one) at `snapshot`, without run
/// recovery, so it reads its own rows only.
fn first_request(limit: u16, snapshot: u64) -> RuntimeEventQueryPageRequest {
    RuntimeEventQueryPageRequest::new(limit, None)
        .and_then(|request| request.at_snapshot(snapshot))
        .expect("a page request")
        .without_run_recovery()
}

fn next_request(page: &RuntimeEventQueryPage, limit: u16) -> Option<RuntimeEventQueryPageRequest> {
    page.next_cursor().map(|cursor| {
        RuntimeEventQueryPageRequest::new(limit, Some(cursor.clone()))
            .expect("a cursor page request")
            .without_run_recovery()
    })
}

fn view_query(view: LedgerView) -> EventQuery {
    EventQuery {
        view: Some(view),
        ..EventQuery::default()
    }
}

// T1: the opening reads the meta row, the aggregates, the tail and the (here empty) eviction rows,
// the same rows on both histories, and no record older than its tail.
#[test]
fn ln_off1_t1_the_opening_reads_the_same_rows_at_both_scales() {
    let mut read = Vec::new();
    for records in [SMALL_RECORDS, LARGE_RECORDS] {
        let root = synthetic_root(records);
        let counter = LedgerReadCounter::start();
        let metadata = offline(root.path(), SYNTHETIC_TAIL).expect("the bounded opening");
        let counts = counter.counts();
        let sequences = counter.sequences_read();
        drop(counter);
        let head = metadata.latest_sequence();
        assert!(head >= records, "the history and the writer's own records");
        assert_eq!(
            metadata.event_count(),
            usize::try_from(head).expect("a count"),
            "1..=head is contiguous"
        );
        assert!(metadata.read_complete() && metadata.corrupt_tail().is_none());
        let tail = (head - SYNTHETIC_TAIL as u64 + 1..=head).collect::<BTreeSet<_>>();
        assert_eq!(
            sequences.events, tail,
            "{records} records: the opening read the tail's records and no other"
        );
        assert_eq!(counts.events.repeated, 0, "{counts:?}");
        assert_eq!(counts.links.repeated, 0, "{counts:?}");
        read.push((
            counts.events.rows,
            counts.links.rows,
            counts.unidentified,
            counts.other_rows,
        ));
    }
    assert_eq!(
        read[0], read[1],
        "the rows the opening read on {SMALL_RECORDS} and on {LARGE_RECORDS} records"
    );
}

// T2: each view's first page and the page after it read at most their rows and lookahead, on
// both histories; across the two pages only the lookahead row of the first (the second's first
// row) is read twice. A view whose first page is full selects the same records of the history's
// start on both (the generator writes the same records in the same order whatever the length,
// and the two pages lie within the records both histories share, `SCALE_PAGE_LIMIT`): it reads
// the same rows and does the same SQLite work on both (at most 1.5 times).
#[test]
fn ln_off1_t2_view_pages_read_the_same_rows_at_both_scales() {
    let small_root = synthetic_root(SMALL_RECORDS);
    let large_root = synthetic_root(LARGE_RECORDS);
    let small = offline(small_root.path(), SYNTHETIC_TAIL).expect("the small opening");
    let large = offline(large_root.path(), SYNTHETIC_TAIL).expect("the large opening");
    let page_rows = 2 * (usize::from(SCALE_PAGE_LIMIT) + 1);
    for view in LedgerView::ALL {
        let mut read = Vec::new();
        for snapshot in [&small, &large] {
            let query = view_query(view);
            let counter = LedgerReadCounter::start();
            let first = snapshot
                .project_view_page(
                    &query,
                    ProjectionProfile::Forensic,
                    &first_request(SCALE_PAGE_LIMIT, snapshot.latest_sequence()),
                )
                .expect("a first page");
            if let Some(next) = next_request(&first, SCALE_PAGE_LIMIT) {
                snapshot
                    .project_view_page(&query, ProjectionProfile::Forensic, &next)
                    .expect("the next page");
            }
            let counts = counter.counts();
            let steps = counter.vm_steps();
            let records = counter.sequences_read().events.len();
            drop(counter);
            assert!(
                records <= page_rows,
                "{view:?}: two pages read {records} records"
            );
            assert!(
                counts.events.repeated <= 1 && counts.links.repeated <= 1,
                "{view:?}: only the lookahead row is read again: {counts:?}"
            );
            assert_eq!(
                first.read_scope().map(|scope| scope.source),
                Some(LedgerReadSource::Offline)
            );
            read.push((
                (counts.events.rows, counts.links.rows, counts.artifacts.rows),
                steps,
                first.has_more(),
            ));
        }
        let ((small_rows, small_steps, full), (large_rows, large_steps, _)) = (read[0], read[1]);
        if full {
            assert_eq!(small_rows, large_rows, "{view:?}: rows read");
            assert!(
                large_steps * 2 <= small_steps * 3,
                "{view:?}: VM steps {small_steps} on {SMALL_RECORDS} records, {large_steps} on \
                 {LARGE_RECORDS}"
            );
        }
    }
}

/// A page as JSON with its read scope's source taken out, and that source.
fn without_source(page: &RuntimeEventQueryPage) -> (Value, Option<LedgerReadSource>) {
    let mut value = serde_json::to_value(page).expect("page JSON");
    if let Some(scope) = value.get_mut("read_scope").and_then(Value::as_object_mut) {
        scope.remove("source");
    }
    (value, page.read_scope().map(|scope| scope.source))
}

// T3: on a synthetic history, each view's first page and the page after it, read offline and by
// the writer at the same point, are equal field by field but for the source.
#[test]
fn ln_off1_t3_an_offline_page_is_the_writers_page_at_the_same_point() {
    let root = synthetic_root(SMALL_RECORDS);
    let database =
        Arc::new(RuntimeStateStore::open_database(root.path(), STATE_KEY).expect("State database"));
    let limits = MaintenanceLimits::default();
    let ledger = LedgerMaintenance::acquire(
        root.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("the writer's owner")
    .open_writer(Arc::clone(&database), OWNER.into())
    .expect("the writer");
    let snapshot = offline(root.path(), SYNTHETIC_TAIL).expect("the bounded opening");
    let head = snapshot.latest_sequence();
    assert_eq!(ledger.latest_sequence().expect("the writer's head"), head);
    for view in LedgerView::ALL {
        let query = view_query(view);
        let mut request = Some(
            RuntimeEventQueryPageRequest::new(PAGE_LIMIT, None)
                .and_then(|request| request.at_snapshot(head))
                .expect("a page request"),
        );
        for _ in 0..2 {
            let Some(asked) = request.take() else {
                break;
            };
            let offline_page = snapshot
                .project_view_page(&query, ProjectionProfile::Forensic, &asked)
                .expect("the offline page");
            let writer_page = ledger
                .project_view_page(query.clone(), ProjectionProfile::Forensic, asked)
                .expect("the writer's page");
            let (offline_value, offline_source) = without_source(&offline_page);
            let (writer_value, writer_source) = without_source(&writer_page);
            assert_eq!(offline_value, writer_value, "{view:?}");
            assert_eq!(offline_source, Some(LedgerReadSource::Offline), "{view:?}");
            assert_eq!(writer_source, Some(LedgerReadSource::Runtime), "{view:?}");
            request = offline_page.next_cursor().map(|cursor| {
                RuntimeEventQueryPageRequest::new(PAGE_LIMIT, Some(cursor.clone()))
                    .expect("a cursor page request")
            });
        }
    }
    ledger.close().expect("close the writer");
}

/// A start's writer open of `root` with a tail of `PLAIN_TAIL` records (as `gate_ln8.rs`).
fn writer_open(
    root: &Path,
    database: &Arc<RuntimeDatabase>,
) -> Result<GlobalLedger, GlobalLedgerError> {
    let state_root = root.canonicalize().expect("state root");
    let plan = LedgerFailurePlan::new(
        LedgerWritePolicy::new(1, vec![0], STALL_MS, PROBE_MS, DUMP_MAX_BYTES)
            .expect("write policy"),
        &state_root,
        None,
        epoch(),
        AT_MS,
    )
    .with_open_verify_tail_records(PLAIN_TAIL);
    let limits = MaintenanceLimits::default();
    LedgerMaintenance::acquire(root, false, limits, limits.deadline().expect("deadline"))?
        .open_writer_timed(
            Arc::clone(database),
            OWNER.into(),
            Some(plan),
            &mut LedgerOpenTiming::default(),
        )
}

fn damage(database: &RuntimeDatabase, sql: &str) {
    database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("fixture connection")
        .execute_batch(sql)
        .expect("damage the fixture");
}

/// The SQL that deletes the record at `sequence` with its link and artifact rows.
fn delete_record(sequence: u64) -> String {
    let stored = stored(sequence);
    format!(
        "DELETE FROM ledger_links WHERE sequence={stored}; DELETE FROM ledger_artifacts WHERE sequence={stored}; DELETE FROM ledger_events WHERE sequence={stored}"
    )
}

// T4: each damage the writer open refuses in its tail or meta row is refused by the offline
// opening with the same code and operation; an older record the opening does not read is
// refused, fatally, by the page that reads it.
#[test]
fn ln_off1_t4_the_opening_refuses_what_the_writer_open_refuses() {
    let cases: [(&str, fn(u64) -> String, &str, &str); 5] = [
        (
            "a tail record's tag",
            |head| {
                format!(
                    "UPDATE ledger_events SET integrity_tag='ln-off1-invalid-tag' WHERE sequence={}",
                    stored(head - 1)
                )
            },
            "ledger_record_mismatch",
            "verify_sqlite_record",
        ),
        (
            "the meta row's tag",
            |_| "UPDATE ledger_meta SET integrity_tag='ln-off1-invalid-meta'".to_owned(),
            "ledger_meta_mismatch",
            "verify_sqlite_head",
        ),
        (
            "the head record",
            delete_record,
            "ledger_meta_mismatch",
            "verify_sqlite_head",
        ),
        (
            "a record past the head",
            |head| {
                format!(
                    "INSERT INTO ledger_events SELECT sequence+1,event_id||'-past',timestamp_unix_ms,event_type,severity,sensitivity,origin_source,origin_module,origin_actor,payload_schema,canonical_record,record_sha256,previous_record_sha256,integrity_tag FROM ledger_events WHERE sequence={}",
                    stored(head)
                )
            },
            "ledger_meta_mismatch",
            "verify_sqlite_head",
        ),
        (
            "a tail record",
            |head| delete_record(head - 3),
            "sequence_discontinuity",
            "recover_sequence",
        ),
    ];
    for (label, sql, code, operation) in cases {
        let (root, database) = plain_root(PLAIN_RECORDS);
        let head = offline(root.path(), PLAIN_TAIL)
            .expect("the undamaged root opens")
            .latest_sequence();
        damage(&database, &sql(head));
        let refused = offline(root.path(), PLAIN_TAIL)
            .err()
            .unwrap_or_else(|| panic!("{label}: the offline opening refuses it"));
        assert_eq!(
            (refused.code(), refused.operation()),
            (code, operation),
            "{label}: offline"
        );
        assert!(refused.is_fatal(), "{label}: {refused}");
        let writer = writer_open(root.path(), &database)
            .err()
            .unwrap_or_else(|| panic!("{label}: the writer open refuses it"));
        assert_eq!(
            (writer.code(), writer.operation()),
            (code, operation),
            "{label}: writer"
        );
    }

    // A deleted record older than the tail: the contiguity check refuses it at the opening.
    let (root, database) = plain_root(PLAIN_RECORDS);
    damage(&database, &delete_record(OLD));
    let refused = offline(root.path(), PLAIN_TAIL)
        .err()
        .expect("the opening finds the gap");
    assert_eq!(
        (refused.code(), refused.operation()),
        ("sequence_discontinuity", "recover_sequence")
    );

    // A damaged record older than the tail: the opening does not read it; the page that does
    // refuses it, fatally.
    let (root, database) = plain_root(PLAIN_RECORDS);
    damage(
        &database,
        &format!(
            "UPDATE ledger_events SET integrity_tag='ln-off1-invalid-tag' WHERE sequence={}",
            stored(OLD)
        ),
    );
    let snapshot = offline(root.path(), PLAIN_TAIL).expect("the opening reads its tail only");
    let head = snapshot.latest_sequence();
    let tail_query = EventQuery {
        from_sequence: Some(head - 1),
        ..EventQuery::default()
    };
    let tail = snapshot
        .project_view_page(
            &tail_query,
            ProjectionProfile::Cli,
            &first_request(10, head),
        )
        .expect("a page of the tail");
    assert_eq!(tail.events().len(), 2);
    let old_query = EventQuery {
        from_sequence: Some(OLD),
        to_sequence: Some(OLD),
        ..EventQuery::default()
    };
    let refused = snapshot
        .project_view_page(&old_query, ProjectionProfile::Cli, &first_request(10, head))
        .expect_err("the page that reads the damaged record refuses it");
    assert_eq!(
        (refused.code(), refused.operation()),
        ("ledger_record_mismatch", "verify_sqlite_record")
    );
    assert!(refused.is_fatal(), "{refused}");
}

fn assert_budget(error: &GlobalLedgerError, operation: &str, phase: &str) {
    assert_eq!(
        (error.code(), error.operation()),
        ("ledger_read_budget_exceeded", operation),
        "{error:?}"
    );
    assert!(!error.is_fatal(), "{error:?}");
    assert!(
        error
            .detail()
            .is_some_and(|detail| detail.contains(&format!("phase={phase}"))),
        "{error:?}"
    );
}

// T5: with the defaults alone (`GlobalLedgerEvidenceConfig::new`), a deadline that passes refuses
// the opening and a page with `ledger_read_budget_exceeded` naming the phase: between statements
// (`read_only_snapshot`) and inside a statement (the read's own operation). An opening whose own
// deadline passed leaves the later reads their own.
#[test]
fn ln_off1_t5_every_read_has_its_own_default_deadline() {
    let (root, _database) = plain_root(PLAIN_RECORDS);
    let page = |snapshot: &GlobalLedgerMetadata| {
        snapshot.project_view_page(
            &EventQuery::default(),
            ProjectionProfile::Cli,
            &first_request(10, snapshot.latest_sequence()),
        )
    };
    let defaults = || GlobalLedgerEvidenceConfig::new(root.path());

    // The opening's deadline passes at its first look.
    {
        let _clock = LedgerReadClock::arm(0);
        let error = GlobalLedger::open_metadata(defaults())
            .err()
            .expect("the opening stops at its deadline");
        assert_budget(&error, "read_only_snapshot", "open");
    }
    // Inside the opening's first statement.
    {
        let _clock = LedgerReadClock::arm(1);
        let error = GlobalLedger::open_metadata(defaults().sqlite_read_check_vm_steps(1))
            .err()
            .expect("the opening stops inside a statement");
        assert_budget(&error, "open_ledger_metadata", "open");
    }
    // A page whose deadline passes at its first look, and inside its first statement.
    let snapshot =
        GlobalLedger::open_metadata(defaults().sqlite_read_check_vm_steps(1)).expect("opening");
    {
        let _clock = LedgerReadClock::arm(0);
        let error = page(&snapshot).expect_err("the page stops at its deadline");
        assert_budget(&error, "read_only_snapshot", "page");
    }
    {
        let _clock = LedgerReadClock::arm(1);
        let error = page(&snapshot).expect_err("the page stops inside a statement");
        assert_budget(&error, "project_ledger_view_page", "page");
    }
    // The same snapshot reads again under the next page's own deadline.
    assert_eq!(page(&snapshot).expect("the next page").events().len(), 10);

    // An opening given a deadline that has passed by the time it ends (the fake clock lets it
    // open): every later read takes its own deadline when it starts and reads, as a UI page read
    // more than the opening's 60 s after it now does.
    let opened = {
        let _never = LedgerReadClock::arm(u64::MAX);
        GlobalLedger::open_metadata(defaults().with_budget(
            1_073_741_824,
            300_000,
            deadline_passed(),
        ))
        .expect("the opening, before its deadline on the fake clock")
    };
    assert_eq!(
        page(&opened)
            .expect("a page after the opening's deadline")
            .events()
            .len(),
        10
    );
}

// T6: a root whose writer appends while the offline snapshot pages: every page is at the pinned
// head and the pages give exactly the records through it; every append commits; once the pages
// are read a passive checkpoint writes back the whole WAL.
#[test]
fn ln_off1_t6_a_live_root_is_read_at_its_pinned_head_without_holding_the_writer() {
    let dir = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(dir.path());
    for index in 0..LIVE_BEFORE {
        ledger
            .append(event(&format!("before-{index}")))
            .expect("record before the opening");
    }
    let snapshot = offline(dir.path(), SYNTHETIC_TAIL).expect("the opening of a live root");
    let pinned = snapshot.latest_sequence();
    assert_eq!(ledger.latest_sequence().expect("head"), pinned);
    let (pages, appended) = std::thread::scope(|scope| {
        let appender = scope.spawn(|| {
            (0..LIVE_APPENDED)
                .map(|index| ledger.append(event(&format!("during-{index}"))).map(|_| ()))
                .collect::<Result<Vec<_>, _>>()
        });
        let mut sequences = Vec::new();
        let mut request = Some(first_request(PAGE_LIMIT, pinned));
        while let Some(asked) = request.take() {
            let page = snapshot
                .project_view_page(&EventQuery::default(), ProjectionProfile::Cli, &asked)
                .expect("a page while the writer appends");
            assert_eq!(page.snapshot_ledger_position(), pinned);
            sequences.extend(page.events().iter().map(|event| event.sequence));
            request = next_request(&page, PAGE_LIMIT);
        }
        (sequences, appender.join().expect("the appender"))
    });
    appended.expect("every append commits beside the pages");
    assert_eq!(pages, (1..=pinned).collect::<Vec<_>>());
    assert_eq!(
        ledger.latest_sequence().expect("head"),
        pinned + LIVE_APPENDED
    );
    let (busy, log, checkpointed): (i64, i64, i64) = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("connection")
        .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("a passive checkpoint");
    assert_eq!(busy, 0);
    assert_eq!(
        log, checkpointed,
        "the checkpoint reached the end of the WAL"
    );
    ledger.close().expect("close the writer");
}

/// The eviction intent of a closed frame, committed by the writer (as `gate_0121_a4.rs`).
fn evict(
    ledger: &GlobalLedger,
    artifacts: &ArtifactStore,
    frame: &Capture,
) -> Box<ArtifactEvictionPermit> {
    let guard = try_artifact_delete_guard(artifacts.root(), &frame.reference)
        .expect("delete guard")
        .expect("the material guard is free");
    match ledger.admit_artifact_eviction(guard).expect("admission") {
        ArtifactEvictionAdmission::Committed(permit) => permit,
        ArtifactEvictionAdmission::Deferred => panic!("the closed frame is admitted"),
    }
}

// T7: on a root with a completed eviction, a kept frame and a pending eviction (the fixture of
// `gate_0121_a4.rs`), the offline snapshot's point read gives each frame the reference, links,
// eviction proof and availability the whole replay gave.
#[test]
fn ln_off1_t7_a_material_fetch_gives_the_proof_the_whole_replay_gave() {
    let dir = tempfile::tempdir().expect("root");
    let (database, ledger) = formal_root(dir.path());
    let artifacts = material_store(dir.path());
    let evicted = capture(&ledger, &artifacts, b"frame evicted with its proof");
    ledger
        .finish_artifact_eviction(evict(&ledger, &artifacts, &evicted))
        .expect("recorded deletion");
    for index in 0..4 {
        ledger
            .append(event(&format!("between-{index}")))
            .expect("history");
    }
    let kept = capture(&ledger, &artifacts, b"frame without an eviction");
    let pending = capture(&ledger, &artifacts, b"frame of a pending eviction");
    drop(evict(&ledger, &artifacts, &pending));
    let head = ledger.latest_sequence().expect("head");
    ledger.close().expect("close with a pending intent");
    drop(database);

    let snapshot = offline(dir.path(), PLAIN_TAIL).expect("the bounded opening");
    assert_eq!(snapshot.latest_sequence(), head);
    let whole_database =
        RuntimeDatabase::open_existing_unchecked(dir.path()).expect("the database");
    let (replayed, _) = crate::global::sqlite::read_metadata(&whole_database, None, None)
        .expect("the whole replay");
    let deadline = deadline_after(ENOUGH_SECONDS);
    for (label, frame) in [
        ("evicted", &evicted),
        ("kept", &kept),
        ("pending", &pending),
    ] {
        let selection = LedgerArtifactSelection {
            event: LedgerEventPosition {
                event_id: frame.verified.event_id,
                sequence: frame.verified.sequence,
            },
            artifact_id: frame.reference.artifact_id,
            snapshot_position: head,
            sha256: frame.reference.sha256.clone(),
            byte_count: frame.reference.byte_count,
            run_id: None,
            frame_id: None,
            request_id: None,
            correlation_id: None,
        };
        let point = snapshot
            .resolve_artifact(&selection, deadline)
            .expect("the point read");
        let whole = crate::global::evidence::resolve_artifact_from_events(
            &replayed, &selection, head, true, deadline, None,
        )
        .expect("the whole replay's resolution");
        assert_eq!(format!("{point:?}"), format!("{whole:?}"), "{label}");
        assert_eq!(
            point.eviction().is_some(),
            label != "kept",
            "{label}: {point:?}"
        );
    }
}

// The bound (Alice, pending decision 115, 10-30: 「我们直接限定UI每次可以读多少就行」): a page
// returns at most its limit, which is at most `MAX_RUNTIME_EVENT_QUERY_EVENTS` (a request for more
// is invalid), says that more is available and gives a cursor; paging with it returns the rest,
// every record once, in ledger order.
#[test]
fn ln_off1_a_read_returns_at_most_the_bound_and_pages_to_the_rest() {
    let (root, _database) = plain_root(BOUND_RECORDS);
    let snapshot = offline(root.path(), PLAIN_TAIL).expect("the bounded opening");
    let head = snapshot.latest_sequence();
    assert!(head > 3 * u64::from(MAX_RUNTIME_EVENT_QUERY_EVENTS));
    assert!(
        RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS + 1, None).is_err(),
        "no page asks for more than the bound"
    );
    let first = snapshot
        .project_view_page(
            &EventQuery::default(),
            ProjectionProfile::Cli,
            &first_request(MAX_RUNTIME_EVENT_QUERY_EVENTS, head),
        )
        .expect("the first page");
    assert_eq!(first.returned_count(), MAX_RUNTIME_EVENT_QUERY_EVENTS);
    assert!(first.has_more() && first.next_cursor().is_some());
    assert!(
        first
            .read_scope()
            .is_some_and(|scope| scope.limits.contains(&LedgerPageLimit::EventCount)),
        "{:?}",
        first.read_scope()
    );
    let mut sequences = first
        .events()
        .iter()
        .map(|event| event.sequence)
        .collect::<Vec<_>>();
    let mut request = next_request(&first, MAX_RUNTIME_EVENT_QUERY_EVENTS);
    while let Some(asked) = request.take() {
        let page = snapshot
            .project_view_page(&EventQuery::default(), ProjectionProfile::Cli, &asked)
            .expect("a cursor page");
        assert!(page.returned_count() <= MAX_RUNTIME_EVENT_QUERY_EVENTS);
        assert_eq!(page.has_more(), page.next_cursor().is_some());
        sequences.extend(page.events().iter().map(|event| event.sequence));
        request = next_request(&page, MAX_RUNTIME_EVENT_QUERY_EVENTS);
    }
    assert_eq!(sequences, (1..=head).collect::<Vec<_>>());
}
