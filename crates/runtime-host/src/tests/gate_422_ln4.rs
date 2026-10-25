// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #422 LN-4 (LEAN-L model §3.2 step 5, §3.6, §5.1 row LN-4; `contracts/ledger-observer.md`
// §11.5, §11.6): the policy host's decision memory, the agent dispatcher and the monitor registry
// load from their tagged records. A start without the marker replays once and writes one value
// record per key, then the marker; a later start loads from the records the index names.
//
// The history is the ledger crate's synthetic ledger (LN-1) with the fixture's own records after
// it: one settled run of a dispatch pair (its decision memory and its activity state), one live
// wake obligation and the monitor registry's import.

use super::lean_scale::{formal_root, scale_instance_id};
use super::*;
use crate::agent_dispatcher::AgentDispatcherState;
use crate::monitor::MonitorRegistry;
use actingcommand_ledger::{
    LedgerIndexRole, LedgerMaintenance, SyntheticInstance, SyntheticLedgerSpec,
    write_synthetic_ledger,
};
use actingcommand_runtime_database::{DatabaseLocation, MaintenanceLimits, RuntimeDatabase};

// The host test configuration's salt (`config`), also the State database's bootstrap seed.
const SALT: &[u8] = b"runtime-host-test-salt";
const SYNTHETIC_RECORDS: u64 = 2_000;
// The `state` keys the three loads read.
const TAGGED_PREFIXES: [&str; 5] = [
    "decision_memory/",
    "activity_memory/",
    "wake/",
    "monitor_import",
    "monitor/",
];

fn start_host(root: &TempDir, clock: &Arc<ManualRuntimeClock>) -> RuntimeHost {
    RuntimeHost::start(
        config(root)
            .with_runtime_clock(clock.clone())
            .with_agent_dispatcher(AgentDispatcherConfig::new(2, 60_000, 2).expect("agent config")),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            scale_instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host")
}

// A closed formal root holding the synthetic history, every record of it before the fixture's
// clock.
fn synthetic_root() -> TempDir {
    let root = TempDir::new().expect("tempdir");
    formal_root(root.path());
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("the root's database");
    let mut spec = SyntheticLedgerSpec::new(
        SYNTHETIC_RECORDS,
        vec![SyntheticInstance {
            alias: POLICY_INSTANCE_ALIAS.to_owned(),
            instance_id: scale_instance_id(),
        }],
    );
    spec.first_timestamp_unix_ms =
        POLICY_NOW_UNIX_MS - (SYNTHETIC_RECORDS + 1_000) * spec.record_interval_ms;
    let generated = write_synthetic_ledger(&database, &spec).expect("the synthetic ledger");
    assert_eq!(generated.records, SYNTHETIC_RECORDS, "{generated:?}");
    root
}

fn control_record_count(host: &RuntimeHost) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::PolicyControlStateRecorded),
        ..EventQuery::default()
    })
    .expect("the control state records")
    .len()
}

// The seeding's markers the ledger holds, each with its report's code.
fn markers(host: &RuntimeHost) -> Vec<Option<String>> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeLifecycleObserved),
        ..EventQuery::default()
    })
    .expect("the lifecycle records")
    .into_iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(actingcommand_contract::RuntimePayload::LifecycleObserved(
            lifecycle,
        )) => match lifecycle.phase() {
            actingcommand_contract::RuntimeLifecyclePhase::TaggedStateEnabled { outcome } => Some(
                outcome
                    .as_ref()
                    .map(|report| report.code().as_str().to_owned()),
            ),
            _ => None,
        },
        _ => None,
    })
    .collect()
}

// The host's three tagged states equal what today's recoveries rebuild from the whole history.
fn assert_equals_todays_recovery(host: &RuntimeHost, start: &str) {
    let (held, replayed, _) = host
        .decision_memory_for_test()
        .expect("the decision memory");
    assert_eq!(held, replayed, "{start}: the decision memory");
    assert_eq!(
        held.len(),
        2,
        "{start}: the pair's and its activity profile's state"
    );
    let (agent_matches, agent_maps) = host
        .agent_dispatcher_state_for_test()
        .expect("the agent dispatcher");
    assert!(agent_matches, "{start}: the agent dispatcher");
    assert_eq!(agent_maps[0], 1, "{start}: the live wake");
    let (monitor, replayed_monitor) = host.monitor_state_for_test().expect("the monitor registry");
    assert_eq!(monitor, replayed_monitor, "{start}: the monitor registry");
    assert!(monitor.1.is_some(), "{start}: the import");
}

// LN-4 test row 1: seed, then load, equals today's recovery (a shadow comparison) on the
// synthetic ledger. The first start of a build that brings the load finds history and no
// marker (the fixture removes the marker row a start of this build wrote, so the history reads
// as an earlier build's): it replays once, writes one value record per key and the marker
// (`ledger_tagged_state_seeded`). The next start loads: it writes no value record, and its
// decision memory, agent dispatcher and monitor registry equal today's whole-history recoveries.
#[test]
fn ln4_a_seed_then_load_equals_todays_recovery_on_the_synthetic_ledger() {
    let root = synthetic_root();
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));

    // The fixture's records: a settled run of one pair, a live wake and the registry's import.
    let host = start_host(&root, &clock);
    host.activate_policy_catalog(&budget_policy_sources(1))
        .expect("activate the catalog");
    let (_, intent, reasons) =
        evaluated_policy_dispatch_at(&host, PolicyTrigger::Recovery, POLICY_NOW_UNIX_MS, 100);
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { .. } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("the admission")
    else {
        panic!("expected the admission")
    };
    clock.advance(75_000);
    host.record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
        .expect("the settlement");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:ln4-live-wake".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::TimelineReached,
        outcome: planning_signal_outcome(PolicyPlanningSignalKind::TimelineReached),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS + 75_000,
        detection_budget: None,
    })
    .expect("the planning signal");
    assert_eq!(
        markers(&host).len(),
        1,
        "a start of this build marks its root"
    );
    host.close().expect("close the host");

    // The history now reads as an earlier build's: no marker.
    let database = RuntimeDatabase::open_existing(root.path(), false).expect("the root's database");
    let removed = database
        .connection(DatabaseLocation::OpenExistingDatabase.location())
        .expect("database connection")
        .execute(
            "DELETE FROM ledger_index WHERE role='lifecycle' AND key='tagged_state_enabled'",
            [],
        )
        .expect("remove the marker row");
    assert_eq!(removed, 1);
    drop(database);

    // Seed: today's recoveries once, one record per key, then the marker.
    let host = start_host(&root, &clock);
    let before = control_record_count(&host);
    assert_eq!(
        markers(&host).last(),
        Some(&Some("ledger_tagged_state_seeded".to_owned())),
        "the seeding's marker with its report"
    );
    assert_equals_todays_recovery(&host, "the seeding start");
    host.close().expect("close the seeding host");

    // Load: the records alone; no value record is written again.
    let host = start_host(&root, &clock);
    assert_eq!(
        control_record_count(&host),
        before,
        "a loading start writes no value record of an unchanged key"
    );
    assert_eq!(markers(&host).len(), 2, "a loading start writes no marker");
    assert_equals_todays_recovery(&host, "the loading start");
    host.close().expect("close the loading host");
}

// LN-4 test row 2: a second start reads no typed history for the tagged state. On the root the
// first test's history leaves (synthetic history, a settled run, a live wake, the import), the
// three loads a marked start runs read nothing but the index rows and the records they name: the
// writer's largest read is at most the rows read (`writer_largest_read_events` <= index rows),
// and no read scans the whole ledger.
#[test]
fn ln4_b_a_loading_start_reads_no_typed_history_for_the_tagged_state() {
    let root = synthetic_root();
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start_host(&root, &clock);
    host.activate_policy_catalog(&budget_policy_sources(1))
        .expect("activate the catalog");
    let (_, intent, reasons) =
        evaluated_policy_dispatch_at(&host, PolicyTrigger::Recovery, POLICY_NOW_UNIX_MS, 100);
    record_policy_approval(&host, &intent);
    let PolicyDispatchAdmission::Granted { .. } = host
        .admit_policy_dispatch(&intent, &reasons, &policy_context(&host, &intent))
        .expect("the admission")
    else {
        panic!("expected the admission")
    };
    clock.advance(75_000);
    host.record_policy_dispatch_outcome(&intent.decision_id, &PolicyExecutionInput::Succeeded)
        .expect("the settlement");
    host.record_policy_planning_signal(PolicyPlanningSignalEventData {
        signal_id: "signal:ln4-load-reads".to_owned(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        task_id: None,
        kind: PolicyPlanningSignalKind::TimelineReached,
        outcome: planning_signal_outcome(PolicyPlanningSignalKind::TimelineReached),
        observed_at_unix_ms: POLICY_NOW_UNIX_MS + 75_000,
        detection_budget: None,
    })
    .expect("the planning signal");
    host.close().expect("close the host");

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
        .open_writer(Arc::clone(&database), "ln4-load-reads".into())
        .expect("the writer");
    let events =
        crate::events::RuntimeEvents::new(SALT, Arc::new(crate::SystemRuntimeClock::new()))
            .expect("runtime events");
    let owner_epoch = *events
        .issuer()
        .mint_owner_epoch()
        .expect("owner epoch")
        .transport();

    let _ = ledger.take_writer_peak();
    MonitorRegistry::load(
        root.path(),
        [POLICY_INSTANCE_ALIAS.to_owned()],
        owner_epoch,
        &ledger,
        &events,
    )
    .expect("the monitor registry loads");
    let decision_memory =
        crate::policy_host::load_decision_memory_for_test(&ledger).expect("the decision memory");
    let agent = AgentDispatcherState::load(&ledger, crate::recovery_read::RECOVERY_PAGE_EVENTS)
        .expect("the agent dispatcher loads");
    let peak = ledger.take_writer_peak();

    let rows = TAGGED_PREFIXES
        .iter()
        .map(|prefix| {
            ledger
                .index_records(LedgerIndexRole::State, &[*prefix])
                .expect("the index rows")
                .records
                .len()
        })
        .sum::<usize>();
    assert_eq!(decision_memory, 2, "the pair's and its activity's records");
    assert_eq!(agent.map_sizes_for_test()[0], 1, "the live wake");
    assert!(rows >= 4, "the rows read: {rows}");
    assert!(
        peak.largest_read_events <= u64::try_from(rows).expect("rows"),
        "the largest read {} is within the {rows} index rows: {peak:?}",
        peak.largest_read_events
    );
    assert_eq!(peak.whole_ledger_queries, 0, "{peak:?}");
    ledger.close().expect("close the writer");
}
