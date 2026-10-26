// SPDX-License-Identifier: AGPL-3.0-only

// #422 LN-8c (LEAN-L model §7.1 A3): the start's runtime-fact recovery reads the record events
// after its snapshot set in bounded pages. Without a set it reads every runtime-fact record of
// the history, and one query of them grew with the history: 8 424 events on the 1M-record
// synthetic ledger (scale run 40257524915). On the synthetic history, which holds no snapshot
// set, on the closed root's writer with no Runtime around it: at every page size the recovery
// reads every runtime-fact event of the history, rebuilds the store that one read of them and
// today's replay rebuild, and no writer command of it returns more than one page; its largest
// read is one page, or the whole selection when that fits one.

use super::lean_scale::{formal_root, scale_instance_id};
use super::*;
use actingcommand_contract::RuntimePayload;
use actingcommand_ledger::{
    LedgerMaintenance, SyntheticInstance, SyntheticLedgerSpec, write_synthetic_ledger,
};
use actingcommand_runtime_database::{MaintenanceLimits, RuntimeDatabase};
use actingcommand_scheduler::facts::RuntimeFactStore;

// The host test configuration's salt (`config`), also the State database's bootstrap seed.
const SALT: &[u8] = b"runtime-host-test-salt";
// A history of a few runs: some two dozen runtime-fact records, several pages of each small size.
const SYNTHETIC_RECORDS: u64 = 2_000;

// A closed formal root holding the synthetic history.
fn synthetic_root() -> TempDir {
    let root = TempDir::new().expect("tempdir");
    formal_root(root.path());
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("the root's database");
    let spec = SyntheticLedgerSpec::new(
        SYNTHETIC_RECORDS,
        vec![SyntheticInstance {
            alias: POLICY_INSTANCE_ALIAS.to_owned(),
            instance_id: scale_instance_id(),
        }],
    );
    let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
    assert_eq!(generated.records, SYNTHETIC_RECORDS, "{generated:?}");
    root
}

#[test]
fn ln8c_the_runtime_fact_recovery_reads_its_records_in_bounded_pages() {
    let root = synthetic_root();
    // The closed root's writer, with no Runtime around it.
    let database = Arc::new(RuntimeStateStore::open_database(root.path(), SALT).expect("database"));
    let limits = MaintenanceLimits::default();
    let maintenance = LedgerMaintenance::acquire(
        root.path(),
        false,
        limits,
        limits.deadline().expect("deadline"),
    )
    .expect("the maintenance owner");
    let ledger = maintenance
        .open_writer(Arc::clone(&database), "ln8c-runtime-facts".into())
        .expect("the writer");
    let events =
        crate::events::RuntimeEvents::new(SALT, Arc::new(crate::SystemRuntimeClock::new()))
            .expect("runtime events");
    let owner_epoch = *events
        .issuer()
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport();
    // A writer command's peak is recorded after its reply; the reply of the next command means
    // it was. Every window below opens and closes behind this barrier.
    let barrier = || {
        ledger.latest_sequence().expect("the writer's barrier");
    };

    // Today's read: one query of every runtime-fact event of the history, and today's replay.
    let selected = ledger
        .query(EventQuery {
            from_sequence: Some(1),
            origin_module: Some(OriginModule::RuntimeFacts),
            ..EventQuery::default()
        })
        .expect("the runtime-fact events");
    assert!(
        selected.len() > 3,
        "the history holds {} runtime-fact events",
        selected.len()
    );
    let mut today = RuntimeFactStore::new();
    for event in &selected {
        let EventPayload::Runtime(RuntimePayload::FactRecorded(payload)) = event.payload() else {
            panic!(
                "the synthetic history's runtime-fact event {} is a record",
                event.sequence()
            );
        };
        today
            .record(payload.record().clone())
            .expect("today's replay");
    }
    let expected = today.records().cloned().collect::<Vec<_>>();
    assert!(!expected.is_empty(), "today's replay keeps the records");

    for page_events in [1, 2, 3, ledger.history_page_events()] {
        barrier();
        let _ = ledger.take_writer_peak();
        let _ = RuntimeHost::take_last_runtime_fact_replay_for_test();
        let records =
            RuntimeHost::recover_runtime_facts_for_test(&ledger, &events, owner_epoch, page_events)
                .expect("the runtime-fact recovery");
        barrier();
        let peak = ledger.take_writer_peak();
        assert_eq!(
            RuntimeHost::take_last_runtime_fact_replay_for_test(),
            Some((1, selected.len())),
            "page size {page_events}: every runtime-fact event of the history"
        );
        assert_eq!(records, expected, "page size {page_events}");
        let page = u64::try_from(page_events.min(selected.len())).expect("a page size");
        assert_eq!(
            peak.largest_read_events, page,
            "page size {page_events}: the largest read is one page: {peak:?}"
        );
        assert_eq!(peak.whole_ledger_queries, 0, "{peak:?}");
    }
    ledger.close().expect("close the writer");
}
